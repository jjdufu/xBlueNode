from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
from fastapi.testclient import TestClient

from main import app

TEST_TOKEN = "step02-ssrf-conn-token-0123456789abcdef"


@pytest.mark.parametrize(
    ("connector_type", "config"),
    [
        ("POSTGRES", {"host": "127.0.0.1", "database": "db"}),
        ("MYSQL", {"host": "10.0.0.5", "database": "db"}),
        ("CLICKHOUSE", {"host": "192.168.1.10"}),
        ("MONGODB", {"host": "172.16.0.2", "database": "db"}),
        ("REDIS", {"host": "127.0.0.1"}),
        ("ELASTICSEARCH", {"hosts": ["http://127.0.0.1:9200"]}),
        ("NEO4J", {"uri": "bolt://127.0.0.1:7687"}),
        ("INFLUXDB", {"url": "http://169.254.169.254:8086"}),
        ("TDENGINE", {"url": "http://127.0.0.1:6041", "database": "iot"}),
        ("IOTDB", {"host": "127.0.0.1"}),
        ("S3", {"bucket": "b", "endpoint_url": "http://127.0.0.1:9000"}),
        ("SFTP", {"host": "10.0.0.9", "root_path": "/data"}),
        ("KAFKA", {"bootstrap_servers": "127.0.0.1:9092"}),
        ("MQTT", {"host": "127.0.0.1"}),
        ("PULSAR", {"service_url": "pulsar://127.0.0.1:6650"}),
        ("REST_API", {"base_url": "http://169.254.169.254/latest"}),
        ("GRAPHQL", {"endpoint": "http://127.0.0.1:4000/graphql"}),
    ],
)
def test_all_network_connectors_block_private_ips_by_default(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    connector_type: str,
    config: dict[str, Any],
) -> None:
    monkeypatch.setenv("AXPOINT_INTERNAL_TOKEN", TEST_TOKEN)
    monkeypatch.setenv("AXPOINT_DATA_ROOT", str(tmp_path))
    client = TestClient(app)

    resp = client.post(
        "/connectors/test",
        headers={"X-AxPoint-Internal-Token": TEST_TOKEN},
        json={
            "connector_type": connector_type,
            "config": config,
            "secrets": {"password": "top-secret-password-val"},
            "allow_private_network": False,
        },
    )
    assert resp.status_code == 403
    body = resp.json()
    assert body["error_code"] == "PERMISSION_DENIED"
    # Secret must never be echoed in error responses
    assert "top-secret-password-val" not in resp.text
