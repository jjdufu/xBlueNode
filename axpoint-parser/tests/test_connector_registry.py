from __future__ import annotations

from pathlib import Path

import pytest
from fastapi.testclient import TestClient

import connectors.registry as reg
from main import app

TEST_TOKEN = "step02-registry-token-0123456789abcdef"


def test_connector_types_returns_all_32_specs(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setenv("AXPOINT_INTERNAL_TOKEN", TEST_TOKEN)
    monkeypatch.setenv("AXPOINT_DATA_ROOT", str(tmp_path))
    client = TestClient(app)

    resp = client.get(
        "/connectors/types",
        headers={"X-AxPoint-Internal-Token": TEST_TOKEN},
    )
    assert resp.status_code == 200
    items = resp.json()
    assert len(items) == 32

    seen_types = {item["connector_type"] for item in items}
    assert len(seen_types) == 32
    for item in items:
        assert item["display_name"]
        assert isinstance(item["config_schema"], dict)
        assert item["image_tier"] in {"lite", "full"}


def test_missing_optional_driver_returns_422(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setenv("AXPOINT_INTERNAL_TOKEN", TEST_TOKEN)
    monkeypatch.setenv("AXPOINT_DATA_ROOT", str(tmp_path))
    monkeypatch.setattr(reg, "is_driver_available", lambda ct: ct != "DAMENG")
    client = TestClient(app)

    resp = client.post(
        "/connectors/test",
        headers={"X-AxPoint-Internal-Token": TEST_TOKEN},
        json={
            "connector_type": "DAMENG",
            "config": {"host": "93.184.216.34", "port": 5236},
            "secrets": {"password": "secret"},
        },
    )
    assert resp.status_code == 422
    assert resp.json()["error_code"] == "CONNECTOR_DRIVER_UNAVAILABLE"
