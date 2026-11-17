from __future__ import annotations

import base64
import importlib.util
import json
import os
import re
import subprocess
import sys
import threading
import urllib.error
import urllib.request
from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from pathlib import Path
from typing import Protocol, cast

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
SINK_SCRIPT = REPO_ROOT / "scripts" / "gate" / "tools" / "webhook_sink.py"
TEST_SECRET = "axp-step08-test-hmac-secret-9876543210"


class _SinkServerInstance(Protocol):
    @property
    def server_port(self) -> int: ...
    def serve_forever(self, poll_interval: float = 0.5) -> None: ...
    def shutdown(self) -> None: ...
    def server_close(self) -> None: ...


class _WebhookSinkModule(Protocol):
    def WebhookSinkServer(
        self, server_address: tuple[str, int], out_path: Path
    ) -> _SinkServerInstance: ...
    def compute_signature(self, secret: str, timestamp: str, raw_body: bytes) -> str: ...
    def verify_sink_file(self, sink_path: Path, secret: str) -> tuple[bool, str]: ...
    def main(self, argv: Sequence[str] | None = None) -> int: ...


def _load_sink_module() -> _WebhookSinkModule:
    spec = importlib.util.spec_from_file_location("webhook_sink", SINK_SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return cast(_WebhookSinkModule, module)


SINK_MOD = _load_sink_module()
NO_PROXY_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


@contextmanager
def _running_sink(out_path: Path) -> Iterator[str]:
    server = SINK_MOD.WebhookSinkServer(("127.0.0.1", 0), out_path)
    thread = threading.Thread(
        target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True
    )
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5.0)


def _http_request(
    url: str,
    *,
    method: str = "GET",
    body: bytes | None = None,
    headers: dict[str, str] | None = None,
) -> tuple[int, bytes]:
    req = urllib.request.Request(url, data=body, headers=headers or {}, method=method)
    with NO_PROXY_OPENER.open(req, timeout=5.0) as resp:
        status = int(resp.status)
        raw = resp.read()
    return status, raw


def _run_verify_cli(sink_file: Path, secret: str) -> subprocess.CompletedProcess[str]:
    env = {**os.environ, "NO_PROXY": "127.0.0.1,localhost"}
    return subprocess.run(
        [sys.executable, str(SINK_SCRIPT), "--verify", str(sink_file), "--secret", secret],
        capture_output=True,
        text=True,
        env=env,
        check=False,
    )


def test_health_and_unknown_get_route(tmp_path: Path) -> None:
    sink_file = tmp_path / "webhook.jsonl"
    with _running_sink(sink_file) as base_url:
        status, body = _http_request(f"{base_url}/health")
        assert status == 200
        assert body == b'{"status":"ok"}'

        with pytest.raises(urllib.error.HTTPError) as exc_info:
            _http_request(f"{base_url}/unknown")
        assert exc_info.value.code == 404


def test_signed_opened_and_resolved_compact_jsonl_and_cli_verify(tmp_path: Path) -> None:
    sink_file = tmp_path / "nested" / "webhook.jsonl"
    with _running_sink(sink_file) as base_url:
        # Intentionally include spaces/newlines and UTF-8 text in raw wire JSON to verify:
        # 1) JSONL record re-serializes payload compactly (`"event":"OPENED"`, no space after `:`)
        # 2) `body_b64` preserves exact wire bytes for HMAC verification.
        opened_raw = (
            '{\n  "event": "OPENED",\n  "title": "仓库低库存",\n  "value": 10\n}'
        ).encode()
        ts_open = "1763339643"
        sig_open = SINK_MOD.compute_signature(TEST_SECRET, ts_open, opened_raw)
        status_open, body_open = _http_request(
            f"{base_url}/hook",
            method="POST",
            body=opened_raw,
            headers={
                "Content-Type": "application/json",
                "X-AxPoint-Timestamp": ts_open,
                "X-AxPoint-Signature": sig_open,
                "X-AxPoint-Event": "OPENED",
                "X-AxPoint-Delivery": "11111111-1111-1111-1111-111111111111",
                "Idempotency-Key": "deliv-open-1",
            },
        )
        assert status_open == 200
        assert body_open == b'{"ok":true}'

        resolved_raw = json.dumps(
            {"event": "RESOLVED", "title": "仓库库存恢复", "value": 500},
            ensure_ascii=False,
        ).encode("utf-8")
        ts_res = "1763339700"
        sig_res = SINK_MOD.compute_signature(TEST_SECRET, ts_res, resolved_raw)
        status_res, body_res = _http_request(
            f"{base_url}/",
            method="POST",
            body=resolved_raw,
            headers={
                "Content-Type": "application/json",
                "X-AxPoint-Timestamp": ts_res,
                "X-AxPoint-Signature": sig_res,
                "X-AxPoint-Event": "RESOLVED",
                "Idempotency-Key": "deliv-res-1",
            },
        )
        assert status_res == 200
        assert body_res == b'{"ok":true}'

    lines = sink_file.read_text(encoding="utf-8").splitlines()
    assert len(lines) == 2

    # Critical Step 08 gate awk assertions: exact compact `"event":"OPENED"` / `"event":"RESOLVED"`
    assert re.search(r'"event":"OPENED"', lines[0]) is not None
    assert re.search(r'"event":\s+"OPENED"', lines[0]) is None
    assert "仓库低库存" in lines[0]

    assert re.search(r'"event":"RESOLVED"', lines[1]) is not None
    assert re.search(r'"event":\s+"RESOLVED"', lines[1]) is None
    assert "仓库库存恢复" in lines[1]

    rec0 = json.loads(lines[0])
    assert rec0["method"] == "POST"
    assert rec0["path"] == "/hook"
    assert base64.b64decode(rec0["body_b64"]) == opened_raw
    assert rec0["headers"]["x-axpoint-signature"] == sig_open
    assert rec0["headers"]["x-axpoint-timestamp"] == ts_open
    assert rec0["headers"]["x-axpoint-event"] == "OPENED"
    assert rec0["headers"]["idempotency-key"] == "deliv-open-1"
    assert rec0["headers"]["content-type"] == "application/json"

    rec1 = json.loads(lines[1])
    assert rec1["path"] == "/"
    assert base64.b64decode(rec1["body_b64"]) == resolved_raw

    verify_proc = _run_verify_cli(sink_file, TEST_SECRET)
    assert verify_proc.returncode == 0, verify_proc.stderr
    assert verify_proc.stdout.strip() == "OK"


def test_invalid_json_post_returns_400(tmp_path: Path) -> None:
    sink_file = tmp_path / "invalid.jsonl"
    with _running_sink(sink_file) as base_url:
        with pytest.raises(urllib.error.HTTPError) as exc_info:
            _http_request(
                f"{base_url}/hook",
                method="POST",
                body=b"{not-valid-json",
                headers={"Content-Type": "application/json"},
            )
        assert exc_info.value.code == 400
    assert not sink_file.exists()


def test_verify_rejects_tampered_body_wrong_secret_missing_headers_and_empty_file(
    tmp_path: Path,
) -> None:
    sink_file = tmp_path / "valid.jsonl"
    raw_body = b'{"event":"OPENED","value":10}'
    ts = "1763339800"
    sig = SINK_MOD.compute_signature(TEST_SECRET, ts, raw_body)
    valid_record = {
        "received_at": "2026-11-17T00:34:03+00:00",
        "method": "POST",
        "path": "/hook",
        "headers": {
            "content-type": "application/json",
            "x-axpoint-timestamp": ts,
            "x-axpoint-signature": sig,
        },
        "body_b64": base64.b64encode(raw_body).decode("ascii"),
        "payload": {"event": "OPENED", "value": 10},
    }
    sink_file.write_text(
        json.dumps(valid_record, ensure_ascii=False, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    assert _run_verify_cli(sink_file, TEST_SECRET).returncode == 0

    # 1. Wrong secret
    wrong_secret_res = _run_verify_cli(sink_file, "wrong-secret-value")
    assert wrong_secret_res.returncode != 0
    assert wrong_secret_res.stdout == ""
    assert "mismatch" in wrong_secret_res.stderr.lower()

    # 2. Tampered body_b64
    tampered_file = tmp_path / "tampered.jsonl"
    tampered_record = {
        **valid_record,
        "body_b64": base64.b64encode(b'{"event":"OPENED","value":999}').decode("ascii"),
    }
    tampered_file.write_text(
        json.dumps(tampered_record, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    tampered_res = _run_verify_cli(tampered_file, TEST_SECRET)
    assert tampered_res.returncode != 0
    assert tampered_res.stdout == ""

    # 3. Missing signature header
    no_sig_file = tmp_path / "no_sig.jsonl"
    no_sig_record = {
        **valid_record,
        "headers": {"content-type": "application/json", "x-axpoint-timestamp": ts},
    }
    no_sig_file.write_text(
        json.dumps(no_sig_record, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    no_sig_res = _run_verify_cli(no_sig_file, TEST_SECRET)
    assert no_sig_res.returncode != 0
    assert no_sig_res.stdout == ""

    # 4. Missing timestamp header
    no_ts_file = tmp_path / "no_ts.jsonl"
    no_ts_record = {
        **valid_record,
        "headers": {"content-type": "application/json", "x-axpoint-signature": sig},
    }
    no_ts_file.write_text(
        json.dumps(no_ts_record, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    assert _run_verify_cli(no_ts_file, TEST_SECRET).returncode != 0

    # 5. Empty file & whitespace-only file & missing file
    empty_file = tmp_path / "empty.jsonl"
    empty_file.write_text("", encoding="utf-8")
    assert _run_verify_cli(empty_file, TEST_SECRET).returncode != 0

    blank_file = tmp_path / "blank.jsonl"
    blank_file.write_text("\n   \n", encoding="utf-8")
    assert _run_verify_cli(blank_file, TEST_SECRET).returncode != 0

    missing_file = tmp_path / "does_not_exist.jsonl"
    assert _run_verify_cli(missing_file, TEST_SECRET).returncode != 0
