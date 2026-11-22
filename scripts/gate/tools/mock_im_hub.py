#!/usr/bin/env python3
"""Deterministic local RFC-6455 WebSocket + HTTP IM hub stub for Step 21 gate tests.

Uses only Python 3.11+ standard library modules (`asyncio`, `base64`, `hashlib`, `json`, `struct`).
"""
from __future__ import annotations

import argparse
import asyncio
import base64
import hashlib
import json
import struct
import urllib.parse
from dataclasses import dataclass, field
from typing import Any

WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


@dataclass
class HubState:
    port: int
    active_writers: set[asyncio.StreamWriter] = field(default_factory=set)
    connect_count: int = 0
    replies: list[dict[str, Any]] = field(default_factory=list)
    progress: list[dict[str, Any]] = field(default_factory=list)
    notices: list[dict[str, Any]] = field(default_factory=list)
    acks: list[dict[str, Any]] = field(default_factory=list)


def encode_ws_frame(payload: bytes, opcode: int = 0x1) -> bytes:
    length = len(payload)
    first_byte = 0x80 | (opcode & 0x0F)
    if length < 126:
        header = struct.pack("!BB", first_byte, length)
    elif length < 65536:
        header = struct.pack("!BBH", first_byte, 126, length)
    else:
        header = struct.pack("!BBQ", first_byte, 127, length)
    return header + payload


async def read_ws_frame(reader: asyncio.StreamReader) -> tuple[int, bytes] | None:
    try:
        head = await reader.readexactly(2)
    except (asyncio.IncompleteReadError, ConnectionError):
        return None
    b1, b2 = head[0], head[1]
    opcode = b1 & 0x0F
    masked = (b2 & 0x80) != 0
    length = b2 & 0x7F
    try:
        if length == 126:
            ext = await reader.readexactly(2)
            (length,) = struct.unpack("!H", ext)
        elif length == 127:
            ext = await reader.readexactly(8)
            (length,) = struct.unpack("!Q", ext)
        mask_key = await reader.readexactly(4) if masked else b""
        raw = await reader.readexactly(length) if length > 0 else b""
    except (asyncio.IncompleteReadError, ConnectionError):
        return None
    if masked:
        unmasked = bytes(raw[i] ^ mask_key[i % 4] for i in range(len(raw)))
        return opcode, unmasked
    return opcode, raw


def record_outbound_payload(state: HubState, obj: dict[str, Any]) -> None:
    msg_kind = str(obj.get("kind") or "")
    if obj.get("code") == 200 and "text" not in obj and "content" not in obj:
        state.acks.append(obj)
        return

    text_val = obj.get("text")
    if not isinstance(text_val, str):
        raw_content = obj.get("content")
        if isinstance(raw_content, str):
            try:
                parsed_content = json.loads(raw_content)
                if isinstance(parsed_content, dict) and isinstance(parsed_content.get("text"), str):
                    text_val = parsed_content["text"]
                else:
                    text_val = raw_content
            except json.JSONDecodeError:
                text_val = raw_content

    if not isinstance(text_val, str) or not text_val:
        return

    entry: dict[str, Any] = {
        "chat_id": obj.get("chat_id") or obj.get("receive_id") or "",
        "text": text_val,
        "is_final": bool(obj.get("is_final", True)),
    }
    if msg_kind == "bind_notice":
        state.notices.append(entry)
    elif entry["is_final"]:
        state.replies.append(entry)
    else:
        state.progress.append(entry)


async def send_http_json(
    writer: asyncio.StreamWriter, status: int, reason: str, body_obj: dict[str, Any]
) -> None:
    raw = json.dumps(body_obj, ensure_ascii=False).encode("utf-8")
    headers = (
        f"HTTP/1.1 {status} {reason}\r\n"
        "Content-Type: application/json; charset=utf-8\r\n"
        f"Content-Length: {len(raw)}\r\n"
        "Connection: close\r\n\r\n"
    ).encode("ascii")
    writer.write(headers + raw)
    await writer.drain()
    writer.close()


async def handle_ws_session(
    state: HubState,
    reader: asyncio.StreamReader,
    writer: asyncio.StreamWriter,
    ws_key: str,
) -> None:
    accept_raw = hashlib.sha1((ws_key + WS_GUID).encode("ascii")).digest()
    accept_b64 = base64.b64encode(accept_raw).decode("ascii")
    handshake = (
        "HTTP/1.1 101 Switching Protocols\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Accept: {accept_b64}\r\n\r\n"
    ).encode("ascii")
    writer.write(handshake)
    await writer.drain()

    state.active_writers.add(writer)
    state.connect_count += 1
    try:
        while True:
            frame = await read_ws_frame(reader)
            if frame is None:
                break
            opcode, payload = frame
            if opcode == 0x8:
                writer.write(encode_ws_frame(b"", opcode=0x8))
                await writer.drain()
                break
            if opcode == 0x9:
                writer.write(encode_ws_frame(payload, opcode=0xA))
                await writer.drain()
                continue
            if opcode in (0x1, 0x2) and payload:
                try:
                    decoded = json.loads(payload.decode("utf-8"))
                except (UnicodeDecodeError, json.JSONDecodeError):
                    continue
                if isinstance(decoded, dict):
                    if decoded.get("cmd") == "ping" or decoded.get("type") == "ping":
                        pong = json.dumps({"type": "pong"}).encode("utf-8")
                        writer.write(encode_ws_frame(pong, opcode=0x1))
                        await writer.drain()
                    elif decoded.get("cmd") == "aibot_subscribe":
                        ack = json.dumps({"errcode": 0, "errmsg": "ok"}).encode("utf-8")
                        writer.write(encode_ws_frame(ack, opcode=0x1))
                        await writer.drain()
                    else:
                        record_outbound_payload(state, decoded)
    except ConnectionError:
        pass
    finally:
        state.active_writers.discard(writer)
        writer.close()


async def broadcast_inbound_event(state: HubState, payload: dict[str, Any]) -> int:
    event_id = str(payload.get("event_id") or "evt_default")
    ext_uid = str(payload.get("external_user_id") or "ou_default")
    ext_name = str(payload.get("external_display_name") or "")
    chat_id = str(payload.get("chat_id") or "oc_default")
    text = str(payload.get("text") or "")
    envelope = {
        "schema": "2.0",
        "header": {
            "event_id": event_id,
            "event_type": "im.message.receive_v1",
        },
        "event": {
            "sender": {
                "sender_id": {"open_id": ext_uid},
                "sender_name": ext_name,
            },
            "message": {
                "message_id": f"om_{event_id}",
                "chat_id": chat_id,
                "chat_type": str(payload.get("chat_type") or "p2p"),
                "message_type": "text",
                "content": json.dumps({"text": text}, ensure_ascii=False),
            },
        },
    }
    frame = encode_ws_frame(json.dumps(envelope, ensure_ascii=False).encode("utf-8"), opcode=0x1)
    delivered = 0
    dead: list[asyncio.StreamWriter] = []
    for ws in list(state.active_writers):
        try:
            ws.write(frame)
            await ws.drain()
            delivered += 1
        except ConnectionError:
            dead.append(ws)
    for ws in dead:
        state.active_writers.discard(ws)
        ws.close()
    return delivered


async def handle_client(
    state: HubState, reader: asyncio.StreamReader, writer: asyncio.StreamWriter
) -> None:
    try:
        raw_head = await reader.readuntil(b"\r\n\r\n")
    except (asyncio.IncompleteReadError, asyncio.LimitOverrunError, ConnectionError):
        writer.close()
        return

    lines = raw_head.decode("iso-8859-1").split("\r\n")
    if not lines or not lines[0]:
        writer.close()
        return
    parts = lines[0].split(" ")
    if len(parts) < 2:
        writer.close()
        return
    method, target = parts[0].upper(), parts[1]
    parsed_url = urllib.parse.urlsplit(target)
    path = parsed_url.path

    headers: dict[str, str] = {}
    for line in lines[1:]:
        if ":" in line:
            k, v = line.split(":", 1)
            headers[k.strip().lower()] = v.strip()

    content_len = int(headers.get("content-length", "0") or "0")
    body_bytes = await reader.readexactly(content_len) if content_len > 0 else b""

    if method == "GET" and path == "/ws" and headers.get("upgrade", "").lower() == "websocket":
        ws_key = headers.get("sec-websocket-key", "")
        if not ws_key:
            await send_http_json(writer, 400, "Bad Request", {"error": "missing websocket key"})
            return
        await handle_ws_session(state, reader, writer, ws_key)
        return

    body_json: dict[str, Any] = {}
    if body_bytes:
        try:
            loaded = json.loads(body_bytes.decode("utf-8"))
            if isinstance(loaded, dict):
                body_json = loaded
        except (UnicodeDecodeError, json.JSONDecodeError):
            pass

    if method == "GET" and path == "/test/health":
        await send_http_json(
            writer,
            200,
            "OK",
            {
                "ok": True,
                "ws_connected": len(state.active_writers) > 0,
                "connect_count": state.connect_count,
            },
        )
        return

    if method == "POST" and path == "/callback/ws/endpoint":
        app_id = str(body_json.get("AppID") or body_json.get("app_id") or "")
        has_cred = bool(app_id) and bool(
            body_json.get("AppSecret") or body_json.get("app_secret")
        )
        if not has_cred:
            await send_http_json(writer, 401, "Unauthorized", {"code": 99991663, "msg": "unauthorized"})
            return
        await send_http_json(
            writer,
            200,
            "OK",
            {
                "code": 0,
                "msg": "ok",
                "data": {
                    "URL": f"ws://127.0.0.1:{state.port}/ws?device_id=gate21&service_id=1",
                    "ClientConfig": {
                        "ReconnectCount": 5,
                        "ReconnectInterval": 1,
                        "ReconnectNonce": 1,
                        "PingInterval": 15,
                    },
                },
            },
        )
        return

    if method == "POST" and path == "/open-apis/auth/v3/tenant_access_token/internal":
        await send_http_json(
            writer,
            200,
            "OK",
            {"code": 0, "msg": "ok", "tenant_access_token": "tat_mock_gate21", "expire": 7200},
        )
        return

    if method == "POST" and path.startswith("/open-apis/im/v1/messages"):
        record_outbound_payload(state, body_json)
        await send_http_json(
            writer,
            200,
            "OK",
            {"code": 0, "msg": "ok", "data": {"message_id": f"om_reply_{len(state.replies)}"}},
        )
        return

    if method == "POST" and path == "/v1.0/gateway/connections/open":
        await send_http_json(
            writer,
            200,
            "OK",
            {"endpoint": f"ws://127.0.0.1:{state.port}/ws", "ticket": "dt_ticket_gate21"},
        )
        return

    if method == "POST" and path == "/test/inject":
        delivered = await broadcast_inbound_event(state, body_json)
        await send_http_json(writer, 200, "OK", {"ok": True, "delivered": delivered})
        return

    if method == "POST" and path == "/test/drop-ws":
        dropped = len(state.active_writers)
        for ws in list(state.active_writers):
            ws.close()
        state.active_writers.clear()
        await send_http_json(writer, 200, "OK", {"ok": True, "dropped": dropped})
        return

    if method == "GET" and path == "/test/replies":
        await send_http_json(
            writer,
            200,
            "OK",
            {
                "items": state.replies,
                "progress": state.progress,
                "notices": state.notices,
                "acks": state.acks,
            },
        )
        return

    await send_http_json(writer, 404, "Not Found", {"error": "not found", "path": path})


async def run_server(port: int) -> None:
    state = HubState(port=port)
    server = await asyncio.start_server(
        lambda r, w: handle_client(state, r, w), host="127.0.0.1", port=port
    )
    async with server:
        await server.serve_forever()


def main() -> None:
    parser = argparse.ArgumentParser(description="Mock Enterprise IM WebSocket Hub")
    parser.add_argument("--port", type=int, default=19821)
    args = parser.parse_args()
    asyncio.run(run_server(args.port))


if __name__ == "__main__":
    main()
