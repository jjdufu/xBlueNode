from __future__ import annotations

from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from main import app
from security import OutboundBlocked, check_outbound_url

TEST_TOKEN = "step01-security-token-0123456789abcdef"
FIXTURES_DIR = Path(__file__).resolve().parent / "fixtures"


@pytest.fixture()
def secured_client(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> TestClient:
    monkeypatch.setenv("AXPOINT_INTERNAL_TOKEN", TEST_TOKEN)
    monkeypatch.setenv("AXPOINT_DATA_ROOT", str(tmp_path))
    return TestClient(app)


def _auth_headers(**extra: str) -> dict[str, str]:
    return {"X-AxPoint-Internal-Token": TEST_TOKEN, **extra}


def test_zip_bomb_returns_413_archive_bomb(secured_client: TestClient) -> None:
    raw = (FIXTURES_DIR / "zip_bomb.xlsx").read_bytes()
    resp = secured_client.post(
        "/parse/unified",
        headers=_auth_headers(),
        files={"file": ("zip_bomb.xlsx", raw, "application/octet-stream")},
    )
    assert resp.status_code == 413
    body = resp.json()
    assert body["error_code"] == "ARCHIVE_BOMB"


@pytest.mark.parametrize("attack_name", ["xxe.xml", "billion_laughs.xml"])
def test_xml_entity_attacks_return_422(secured_client: TestClient, attack_name: str) -> None:
    raw = (FIXTURES_DIR / attack_name).read_bytes()
    resp = secured_client.post(
        "/parse/unified",
        headers=_auth_headers(),
        files={"file": (attack_name, raw, "application/xml")},
    )
    assert resp.status_code == 422
    assert resp.json()["error_code"] == "XML_UNSAFE"


def test_upload_exceeding_max_bytes_returns_413(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setenv("AXPOINT_INTERNAL_TOKEN", TEST_TOKEN)
    monkeypatch.setenv("AXPOINT_DATA_ROOT", str(tmp_path))
    monkeypatch.setenv("AXPOINT_UPLOAD_MAX_BYTES", "512")
    client = TestClient(app)

    oversized = b"a,b\n" + (b"1,2\n" * 300)
    resp = client.post(
        "/parse/unified",
        headers=_auth_headers(),
        files={"file": ("big.csv", oversized, "text/csv")},
    )
    assert resp.status_code == 413
    assert resp.json()["error_code"] == "UPLOAD_TOO_LARGE"


@pytest.mark.parametrize("bad_tenant", ["../../etc", "../root", "BAD_UPPER", "a/b", "t..1"])
def test_path_traversal_tenant_header_returns_400(
    secured_client: TestClient, bad_tenant: str
) -> None:
    resp = secured_client.post(
        "/parse/unified",
        headers=_auth_headers(**{"X-AxPoint-Tenant": bad_tenant}),
        files={"file": ("ok.csv", b"a,b\n1,2\n", "text/csv")},
    )
    assert resp.status_code == 400
    assert resp.json()["error_code"] == "INVALID_ARGUMENT"


@pytest.mark.parametrize(
    "forbidden_url",
    [
        "http://127.0.0.1:8080/api",
        "http://169.254.169.254/latest/meta-data/",
        "http://10.0.0.1/internal",
        "http://172.16.0.1/internal",
        "http://192.168.1.1/admin",
        "http://100.64.0.1/cgnat",
        "http://[::1]:8000/loopback",
        "http://[fe80::1]/link-local",
        "http://[fc00::1]/ula",
        "file:///etc/passwd",
    ],
)
def test_ssrf_blocks_private_and_metadata_urls(forbidden_url: str) -> None:
    with pytest.raises(OutboundBlocked):
        check_outbound_url(forbidden_url, allow_private_network=False)


def test_ssrf_still_blocks_cloud_metadata_when_private_allowed() -> None:
    with pytest.raises(OutboundBlocked):
        check_outbound_url("http://169.254.169.254/latest/meta-data/", allow_private_network=True)

    # Loopback and RFC1918 are permitted only when allow_private_network=True
    assert (
        check_outbound_url("http://127.0.0.1:5432/", allow_private_network=True)
        == "http://127.0.0.1:5432/"
    )
    assert (
        check_outbound_url("http://93.184.216.34/v1/items", allow_private_network=False)
        == "http://93.184.216.34/v1/items"
    )
