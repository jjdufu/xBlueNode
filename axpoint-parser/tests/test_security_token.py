from __future__ import annotations

import logging
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from main import app
from settings import REDACTED_VALUE, SecretFilter, Settings
from tests.fixtures.make_fixtures import PARSER_ROOT, generate_all

TEST_INTERNAL_TOKEN = "axp-step00-internal-token-0123456789abcdef"


def _ensure_plain_txt() -> Path:
    plain_path = PARSER_ROOT / "tests" / "fixtures" / "plain.txt"
    if not plain_path.is_file():
        generate_all(root_dir=PARSER_ROOT, convert_legacy=False)
    return plain_path


def test_health_public_without_token(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("AXPOINT_INTERNAL_TOKEN", raising=False)
    client = TestClient(app)

    response = client.get("/health")
    assert response.status_code == 200
    payload = response.json()
    assert payload["status"] == "ok"
    assert payload["service"] == "axpoint-parser"


def test_parse_without_or_invalid_token_returns_401(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AXPOINT_INTERNAL_TOKEN", TEST_INTERNAL_TOKEN)
    plain_path = _ensure_plain_txt()
    raw_bytes = plain_path.read_bytes()
    client = TestClient(app)

    traceparent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
    missing_resp = client.post(
        "/parse",
        files={"file": ("plain.txt", raw_bytes, "text/plain")},
        headers={"traceparent": traceparent},
    )
    assert missing_resp.status_code == 401
    missing_body = missing_resp.json()
    assert set(missing_body.keys()) == {"error_code", "message", "trace_id"}
    assert missing_body["error_code"] == "TOKEN_INVALID"
    assert missing_body["message"] == "internal token required"
    assert missing_body["trace_id"] == traceparent

    wrong_resp = client.post(
        "/parse",
        files={"file": ("plain.txt", raw_bytes, "text/plain")},
        headers={"x-axpoint-internal-token": "wrong-token-value-000000000000000000"},
    )
    assert wrong_resp.status_code == 401
    wrong_body = wrong_resp.json()
    assert wrong_body == {
        "error_code": "TOKEN_INVALID",
        "message": "internal token required",
        "trace_id": "",
    }


def test_parse_with_valid_token_returns_200(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AXPOINT_INTERNAL_TOKEN", TEST_INTERNAL_TOKEN)
    plain_path = _ensure_plain_txt()
    client = TestClient(app)

    with plain_path.open("rb") as fh:
        response = client.post(
            "/parse",
            files={"file": ("plain.txt", fh, "text/plain")},
            headers={"x-axpoint-internal-token": TEST_INTERNAL_TOKEN},
        )

    assert response.status_code == 200
    body = response.json()
    assert body["title"] == "plain"
    assert body["engine"] == "plain-utf8"
    assert "第1段" in body["markdown"]
    assert "第30段" in body["markdown"]


def test_unified_error_envelope_and_secret_redaction(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AXPOINT_INTERNAL_TOKEN", TEST_INTERNAL_TOKEN)
    client = TestClient(app)
    auth_headers = {
        "x-axpoint-internal-token": TEST_INTERNAL_TOKEN,
        "x-request-id": "req-step00-001",
    }

    not_found = client.get("/does-not-exist", headers=auth_headers)
    assert not_found.status_code == 404
    assert not_found.json() == {
        "error_code": "ROUTE_NOT_FOUND",
        "message": "Not Found",
        "trace_id": "req-step00-001",
    }

    empty_upload = client.post(
        "/parse",
        files={"file": ("empty.txt", b"", "text/plain")},
        headers=auth_headers,
    )
    assert empty_upload.status_code == 400
    empty_body = empty_upload.json()
    assert set(empty_body.keys()) == {"error_code", "message", "trace_id"}
    assert empty_body["error_code"] == "INVALID_ARGUMENT"
    assert empty_body["trace_id"] == "req-step00-001"

    cfg = Settings(
        axpoint_internal_token=TEST_INTERNAL_TOKEN,
        axpoint_lake_catalog_dsn="postgres://axpoint_lake:s3cr3t_pw@db.internal:5433/xbluenode",
    )
    assert (
        cfg.lake_catalog_libpq()
        == "dbname=xbluenode host=db.internal port=5433 user=axpoint_lake password=s3cr3t_pw"
    )

    flt = SecretFilter()
    record = logging.LogRecord(
        name="axpoint.test",
        level=logging.INFO,
        pathname=__file__,
        lineno=1,
        msg="connect dsn=postgres://axp:my_super_secret@127.0.0.1:5432/db token=abc123xyz",
        args=(),
        exc_info=None,
    )
    record.__dict__["x_axpoint_internal_token"] = TEST_INTERNAL_TOKEN
    assert flt.filter(record) is True
    rendered = record.getMessage()
    assert "my_super_secret" not in rendered
    assert "abc123xyz" not in rendered
    assert REDACTED_VALUE in rendered
    assert record.__dict__["x_axpoint_internal_token"] == REDACTED_VALUE
