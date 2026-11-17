#!/usr/bin/env python3
"""Step 08 gate HTTP webhook sink server and HMAC-SHA256 JSONL verifier.

Standard-library-only utility invoked by ``scripts/seed/08-rules.sh`` (HTTP sink mode)
and ``scripts/gate/step08.sh`` (signature verification mode).
"""
from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import hmac
import json
import signal
import sys
import threading
from collections.abc import Sequence
from datetime import UTC, datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def compute_signature(secret: str, timestamp: str, raw_body: bytes) -> str:
    """Compute X-AxPoint-Signature header value per Blueprint §08.4."""
    digest = hmac.new(
        secret.encode("utf-8"),
        timestamp.encode("utf-8") + b"." + raw_body,
        hashlib.sha256,
    ).hexdigest()
    return f"sha256={digest}"


class WebhookSinkServer(ThreadingHTTPServer):
    """Thread-safe local HTTP webhook receiver writing compact JSONL records."""

    allow_reuse_address = True
    daemon_threads = True

    def __init__(self, server_address: tuple[str, int], out_path: Path) -> None:
        self.out_path = out_path
        self.write_lock = threading.Lock()
        self.out_path.parent.mkdir(parents=True, exist_ok=True)
        super().__init__(server_address, WebhookSinkHandler)


class WebhookSinkHandler(BaseHTTPRequestHandler):
    """HTTP handler exposing GET /health and POST capture on any route."""

    def do_GET(self) -> None:
        route = self.path.split("?", 1)[0]
        if route == "/health":
            self._send_json(200, {"status": "ok"})
            return
        self._send_json(404, {"error": "not_found"})

    def do_POST(self) -> None:
        raw_len = self.headers.get("Content-Length")
        if raw_len is None:
            self._send_json(400, {"error": "missing_content_length"})
            return
        try:
            content_length = int(raw_len)
        except ValueError:
            self._send_json(400, {"error": "invalid_content_length"})
            return
        if content_length < 0:
            self._send_json(400, {"error": "invalid_content_length"})
            return

        raw_body = self.rfile.read(content_length)
        if len(raw_body) != content_length:
            self._send_json(400, {"error": "incomplete_body"})
            return

        try:
            payload: object = json.loads(raw_body.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError):
            self._send_json(400, {"error": "invalid_json"})
            return

        lower_headers: dict[str, str] = {}
        for key, value in self.headers.items():
            lower_key = key.lower()
            if lower_key in lower_headers:
                lower_headers[lower_key] = f"{lower_headers[lower_key]}, {value}"
            else:
                lower_headers[lower_key] = value

        record: dict[str, object] = {
            "received_at": datetime.now(UTC).isoformat(),
            "method": "POST",
            "path": self.path,
            "headers": lower_headers,
            "body_b64": base64.b64encode(raw_body).decode("ascii"),
            "payload": payload,
        }
        line = json.dumps(record, ensure_ascii=False, separators=(",", ":"))

        if not isinstance(self.server, WebhookSinkServer):
            self._send_json(500, {"error": "invalid_server"})
            return

        with self.server.write_lock:
            self.server.out_path.parent.mkdir(parents=True, exist_ok=True)
            with self.server.out_path.open("a", encoding="utf-8") as fh:
                fh.write(line + "\n")
                fh.flush()

        self._send_json(200, {"ok": True})

    def _send_json(self, status: int, payload: dict[str, object]) -> None:
        body = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.close_connection = True

    def log_message(self, format: str, *args: object) -> None:
        return


def verify_sink_file(sink_path: Path, secret: str) -> tuple[bool, str]:
    """Verify every JSONL record in ``sink_path`` carries a valid HMAC-SHA256 signature."""
    if not secret:
        return False, "webhook_sink: --secret must be non-empty"
    if not sink_path.is_file():
        return False, f"webhook_sink: sink file not found: {sink_path}"

    try:
        content = sink_path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        return False, f"webhook_sink: failed reading {sink_path}: {exc}"

    non_empty = [
        (idx, raw_line)
        for idx, raw_line in enumerate(content.splitlines(), start=1)
        if raw_line.strip()
    ]
    if not non_empty:
        return False, f"webhook_sink: sink file contains no JSONL records: {sink_path}"

    for line_no, raw_line in non_empty:
        try:
            rec = json.loads(raw_line)
        except json.JSONDecodeError as exc:
            return False, f"webhook_sink: line {line_no}: invalid JSON: {exc}"
        if not isinstance(rec, dict):
            return False, f"webhook_sink: line {line_no}: record must be a JSON object"

        body_b64 = rec.get("body_b64")
        if not isinstance(body_b64, str):
            return False, f"webhook_sink: line {line_no}: missing string body_b64"
        try:
            raw_body = base64.b64decode(body_b64, validate=True)
        except (binascii.Error, ValueError) as exc:
            return False, f"webhook_sink: line {line_no}: invalid base64 body_b64: {exc}"

        raw_headers = rec.get("headers")
        if not isinstance(raw_headers, dict):
            return False, f"webhook_sink: line {line_no}: missing headers object"
        headers = {
            k.lower(): v
            for k, v in raw_headers.items()
            if isinstance(k, str) and isinstance(v, str)
        }

        ts = headers.get("x-axpoint-timestamp", "")
        sig = headers.get("x-axpoint-signature", "")
        if not ts:
            return False, f"webhook_sink: line {line_no}: missing x-axpoint-timestamp header"
        if not sig.startswith("sha256=") or len(sig) <= len("sha256="):
            return (
                False,
                f"webhook_sink: line {line_no}: missing or malformed x-axpoint-signature header",
            )

        expected = compute_signature(secret, ts, raw_body)
        if not hmac.compare_digest(sig.encode("utf-8"), expected.encode("utf-8")):
            return False, f"webhook_sink: line {line_no}: HMAC signature mismatch"

    return True, "OK"


def run_server(host: str, port: int, out_path: Path) -> int:
    """Start the HTTP webhook sink and serve until SIGINT or SIGTERM."""
    server = WebhookSinkServer((host, port), out_path)
    shutdown_started = threading.Event()

    def _handle_signal(_signum: int, _frame: object) -> None:
        if not shutdown_started.is_set():
            shutdown_started.set()
            threading.Thread(target=server.shutdown, daemon=True).start()

    is_main = threading.current_thread() is threading.main_thread()
    prev_sigint = signal.getsignal(signal.SIGINT) if is_main else None
    prev_sigterm = signal.getsignal(signal.SIGTERM) if is_main else None
    if is_main:
        signal.signal(signal.SIGINT, _handle_signal)
        signal.signal(signal.SIGTERM, _handle_signal)

    try:
        server.serve_forever(poll_interval=0.1)
    except KeyboardInterrupt:
        pass
    finally:
        if is_main:
            if prev_sigint is not None:
                signal.signal(signal.SIGINT, prev_sigint)
            if prev_sigterm is not None:
                signal.signal(signal.SIGTERM, prev_sigterm)
        server.server_close()
    return 0


def build_parser() -> argparse.ArgumentParser:
    """Construct CLI argument parser for server and verification modes."""
    parser = argparse.ArgumentParser(
        description="Step 08 local HTTP webhook sink and HMAC-SHA256 JSONL verifier."
    )
    parser.add_argument("--host", default="127.0.0.1", help="Bind host (default: 127.0.0.1)")
    parser.add_argument("--port", type=int, default=18900, help="Bind port (default: 18900)")
    parser.add_argument("--out", type=Path, help="Output JSONL path for HTTP sink server mode")
    parser.add_argument("--verify", type=Path, help="JSONL file path to verify HMAC signatures")
    parser.add_argument("--secret", default="", help="HMAC signing secret for --verify mode")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """CLI entrypoint returning process exit status."""
    args = build_parser().parse_args(argv)

    if args.verify is not None:
        ok, message = verify_sink_file(args.verify, args.secret)
        if not ok:
            print(message, file=sys.stderr)
            return 1
        print("OK")
        return 0

    if args.out is None:
        print("webhook_sink: --out is required in HTTP server mode", file=sys.stderr)
        return 1
    if not (0 <= args.port <= 65535):
        print(f"webhook_sink: invalid --port {args.port}", file=sys.stderr)
        return 1

    return run_server(args.host, args.port, args.out)


if __name__ == "__main__":
    sys.exit(main())
