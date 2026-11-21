from __future__ import annotations

from fastapi.testclient import TestClient

from main import app


def test_health_includes_non_empty_version() -> None:
    client = TestClient(app)
    response = client.get("/health")
    assert response.status_code == 200

    payload = response.json()
    assert payload["status"] == "ok"
    assert payload["service"] == "axpoint-parser"
    version = payload.get("version")
    assert isinstance(version, str)
    assert version.strip() != ""
    assert version == app.version
