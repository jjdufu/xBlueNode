from __future__ import annotations

import atexit
import base64
import logging
import os
import queue
import re
import selectors
import subprocess
import threading
import time
import uuid
from collections.abc import Sequence
from pathlib import Path
from typing import Any

import orjson

from schemas import RenderChartRequest
from security import SidecarError
from settings import get_chart_workers

logger = logging.getLogger(__name__)

DEFAULT_CHART_TIMEOUT_S = 15.0
RENDER_SCRIPT_PATH = Path(__file__).resolve().parent / "render.mjs"

_SUSPICIOUS_OPTION_STR_RE = re.compile(
    r"(?i)(?:^\s*function\s*\(|\([^)]*\)\s*=>|^[A-Za-z_$][A-Za-z0-9_$]*\s*=>|<script\b|\bjavascript\s*:)"
)
_FORBIDDEN_KEYS = frozenset({"__proto__", "constructor", "prototype"})


def validate_chart_option(value: Any, *, key_name: str = "", depth: int = 0) -> None:
    """Reject callable objects, non-string formatters, prototype keys, or script strings."""
    if depth > 32:
        raise SidecarError(
            400, "INVALID_ARGUMENT", "chart option exceeds maximum nesting depth of 32"
        )
    if value is None or isinstance(value, (bool, int, float)):
        return
    if callable(value):
        raise SidecarError(400, "INVALID_ARGUMENT", "functions are forbidden in chart option")
    if key_name == "formatter" and not isinstance(value, str):
        raise SidecarError(400, "INVALID_ARGUMENT", "formatter must be a template string")
    if isinstance(value, str):
        if _SUSPICIOUS_OPTION_STR_RE.search(value):
            raise SidecarError(
                400,
                "INVALID_ARGUMENT",
                "executable expressions are forbidden in chart option",
            )
        return
    if isinstance(value, (list, tuple)):
        for item in value:
            validate_chart_option(item, depth=depth + 1)
        return
    if isinstance(value, dict):
        for raw_k, raw_v in value.items():
            k_str = str(raw_k)
            if k_str in _FORBIDDEN_KEYS:
                raise SidecarError(
                    400,
                    "INVALID_ARGUMENT",
                    f"forbidden key in chart option: {k_str!r}",
                )
            validate_chart_option(raw_v, key_name=k_str, depth=depth + 1)
        return
    raise SidecarError(
        400,
        "INVALID_ARGUMENT",
        f"unsupported value type in chart option: {type(value).__name__}",
    )


class _ChartWorker:
    """Wrapper around a single persistent `node charts/render.mjs` subprocess."""

    def __init__(self, cmd: Sequence[str], cwd: Path) -> None:
        self._cmd = list(cmd)
        self._cwd = cwd
        self._proc: subprocess.Popen[bytes] | None = None
        self._rx_buf = bytearray()
        self._spawn()

    @property
    def pid(self) -> int | None:
        return self._proc.pid if self._proc is not None else None

    def _spawn(self) -> None:
        self.kill()
        try:
            self._proc = subprocess.Popen(
                self._cmd,
                cwd=str(self._cwd),
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                bufsize=0,
            )
        except OSError as exc:
            raise SidecarError(
                502,
                "UPSTREAM_PARSER",
                f"failed to start chart renderer worker: {exc}",
            ) from exc
        self._rx_buf.clear()

    def ensure_alive(self) -> None:
        if self._proc is None or self._proc.poll() is not None:
            self._spawn()

    def kill(self) -> None:
        proc = self._proc
        self._proc = None
        self._rx_buf.clear()
        if proc is None:
            return
        for pipe in (proc.stdin, proc.stdout):
            if pipe is not None:
                try:
                    pipe.close()
                except OSError:
                    pass
        if proc.poll() is None:
            try:
                proc.kill()
            except OSError:
                pass
        try:
            proc.wait(timeout=5.0)
        except subprocess.TimeoutExpired:
            logger.warning("Chart worker pid=%s did not exit promptly after SIGKILL", proc.pid)

    def request(self, payload: dict[str, Any], *, deadline: float) -> dict[str, Any]:
        self.ensure_alive()
        proc = self._proc
        if proc is None or proc.stdin is None or proc.stdout is None:
            raise SidecarError(502, "UPSTREAM_PARSER", "chart worker pipes unavailable")

        line_bytes = orjson.dumps(payload) + b"\n"
        try:
            proc.stdin.write(line_bytes)
            proc.stdin.flush()
        except BrokenPipeError as exc:
            # Worker exited while idle; respawn once and retry if time remains.
            if time.monotonic() >= deadline:
                self.kill()
                raise SidecarError(504, "UPSTREAM_TIMEOUT", "chart render timed out") from exc
            self._spawn()
            proc = self._proc
            if proc is None or proc.stdin is None or proc.stdout is None:
                raise SidecarError(
                    502, "UPSTREAM_PARSER", "chart worker pipes unavailable"
                ) from exc
            try:
                proc.stdin.write(line_bytes)
                proc.stdin.flush()
            except OSError as retry_exc:
                self.kill()
                raise SidecarError(
                    502,
                    "UPSTREAM_PARSER",
                    f"failed to write to chart worker: {retry_exc}",
                ) from retry_exc
        except OSError as exc:
            self.kill()
            raise SidecarError(
                502, "UPSTREAM_PARSER", f"failed to write to chart worker: {exc}"
            ) from exc

        stdout_pipe = proc.stdout
        if stdout_pipe is None:
            raise SidecarError(502, "UPSTREAM_PARSER", "chart worker stdout unavailable")
        stdout_fd = stdout_pipe.fileno()
        with selectors.DefaultSelector() as sel:
            sel.register(stdout_fd, selectors.EVENT_READ)
            while True:
                nl_idx = self._rx_buf.find(b"\n")
                if nl_idx != -1:
                    raw_line = bytes(self._rx_buf[:nl_idx])
                    del self._rx_buf[: nl_idx + 1]
                    if not raw_line.strip():
                        continue
                    try:
                        parsed = orjson.loads(raw_line)
                    except orjson.JSONDecodeError as exc:
                        self._spawn()
                        raise SidecarError(
                            502,
                            "UPSTREAM_PARSER",
                            f"malformed JSON from chart worker: {exc}",
                        ) from exc
                    if not isinstance(parsed, dict):
                        self._spawn()
                        raise SidecarError(
                            502, "UPSTREAM_PARSER", "invalid response object from chart worker"
                        )
                    return parsed

                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    self._spawn()
                    raise SidecarError(504, "UPSTREAM_TIMEOUT", "chart render timed out")

                events = sel.select(timeout=remaining)
                if not events:
                    self._spawn()
                    raise SidecarError(504, "UPSTREAM_TIMEOUT", "chart render timed out")

                try:
                    chunk = os.read(stdout_fd, 65_536)
                except OSError as exc:
                    self._spawn()
                    raise SidecarError(
                        502, "UPSTREAM_PARSER", f"failed reading chart worker output: {exc}"
                    ) from exc

                if not chunk:
                    self._spawn()
                    raise SidecarError(
                        502, "UPSTREAM_PARSER", "chart worker exited unexpectedly"
                    )
                self._rx_buf.extend(chunk)


class ChartRendererPool:
    """Thread-safe pool of persistent Node.js ECharts SSR subprocess workers."""

    def __init__(
        self,
        num_workers: int | None = None,
        *,
        worker_cmd: Sequence[str] | None = None,
        cwd: Path | None = None,
    ) -> None:
        self._num_workers = max(1, num_workers if num_workers is not None else get_chart_workers())
        self._cwd = cwd or RENDER_SCRIPT_PATH.parent
        self._cmd: list[str] = (
            list(worker_cmd)
            if worker_cmd is not None
            else ["node", str(RENDER_SCRIPT_PATH)]
        )
        self._lock = threading.Lock()
        self._idle: queue.Queue[_ChartWorker] = queue.Queue()
        self._workers: list[_ChartWorker] = []
        self._started = False
        self._closed = False

    def _ensure_started(self) -> None:
        if self._started and not self._closed:
            return
        with self._lock:
            if self._started and not self._closed:
                return
            self._closed = False
            self._workers.clear()
            while not self._idle.empty():
                try:
                    self._idle.get_nowait()
                except queue.Empty:
                    break
            for _ in range(self._num_workers):
                worker = _ChartWorker(self._cmd, self._cwd)
                self._workers.append(worker)
                self._idle.put(worker)
            self._started = True

    def active_pids(self) -> list[int]:
        """Return PIDs of all currently alive workers in the pool (for diagnostics/tests)."""
        self._ensure_started()
        with self._lock:
            return [w.pid for w in self._workers if w.pid is not None]

    def render(
        self,
        req: RenderChartRequest,
        *,
        timeout_s: float = DEFAULT_CHART_TIMEOUT_S,
    ) -> tuple[bytes, str]:
        """Render an ECharts option to `(image_bytes, mime_type)` within `timeout_s`."""
        if timeout_s <= 0:
            raise SidecarError(400, "INVALID_ARGUMENT", "timeout_s must be positive")

        validate_chart_option(req.option)
        self._ensure_started()

        deadline = time.monotonic() + timeout_s
        try:
            worker = self._idle.get(timeout=timeout_s)
        except queue.Empty as exc:
            raise SidecarError(504, "UPSTREAM_TIMEOUT", "chart render timed out") from exc

        try:
            req_id = uuid.uuid4().hex
            resp = worker.request(
                {
                    "id": req_id,
                    "option": req.option,
                    "width": req.width,
                    "height": req.height,
                    "format": req.format,
                    "theme": req.theme,
                },
                deadline=deadline,
            )
        finally:
            with self._lock:
                if self._closed:
                    worker.kill()
                else:
                    self._idle.put(worker)

        if resp.get("id") != req_id:
            # Desynchronized stream: recycle worker on next borrow and fail safely.
            worker.kill()
            raise SidecarError(502, "UPSTREAM_PARSER", "chart worker response ID mismatch")

        if not resp.get("ok"):
            code = str(resp.get("code") or "UPSTREAM_PARSER")
            err_msg = str(resp.get("error") or "chart render failed")
            status = 400 if code == "INVALID_ARGUMENT" else 502
            raise SidecarError(status, code, err_msg)

        data_b64 = resp.get("data_b64")
        mime = str(resp.get("mime") or ("image/svg+xml" if req.format == "svg" else "image/png"))
        if not isinstance(data_b64, str) or not data_b64:
            raise SidecarError(502, "UPSTREAM_PARSER", "chart worker returned empty payload")

        try:
            raw_bytes = base64.b64decode(data_b64)
        except ValueError as exc:
            raise SidecarError(502, "UPSTREAM_PARSER", f"invalid base64 image data: {exc}") from exc
        return raw_bytes, mime

    def shutdown(self) -> None:
        """Terminate and reap all Node.js chart worker subprocesses."""
        with self._lock:
            self._closed = True
            self._started = False
            workers = list(self._workers)
            self._workers.clear()
            while not self._idle.empty():
                try:
                    self._idle.get_nowait()
                except queue.Empty:
                    break
        for worker in workers:
            worker.kill()


_pool_lock = threading.Lock()
_default_pool: ChartRendererPool | None = None


def get_chart_pool() -> ChartRendererPool:
    """Return process-wide singleton ChartRendererPool."""
    global _default_pool
    if _default_pool is not None:
        return _default_pool
    with _pool_lock:
        if _default_pool is None:
            _default_pool = ChartRendererPool()
        return _default_pool


def render_chart_bytes(
    req: RenderChartRequest,
    timeout_s: float = DEFAULT_CHART_TIMEOUT_S,
) -> tuple[bytes, str]:
    """Render `RenderChartRequest` via singleton pool and return `(bytes, mime_type)`."""
    return get_chart_pool().render(req, timeout_s=timeout_s)


def shutdown_chart_pool() -> None:
    """Shut down singleton ChartRendererPool cleanly (safe to call multiple times)."""
    global _default_pool
    with _pool_lock:
        pool = _default_pool
        _default_pool = None
    if pool is not None:
        pool.shutdown()


atexit.register(shutdown_chart_pool)
