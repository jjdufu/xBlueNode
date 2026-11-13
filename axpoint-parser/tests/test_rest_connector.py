from __future__ import annotations

import httpx
import pytest
import respx

from connectors.rest_api import RestApiConnector
from security import OutboundBlocked

BASE_URL = "https://93.184.216.34"


@respx.mock
def test_rest_page_number_and_offset_pagination() -> None:
    respx.get(f"{BASE_URL}/orders").mock(
        side_effect=[
            httpx.Response(200, json={"items": [{"id": 1}, {"id": 2}]}),
            httpx.Response(200, json={"items": [{"id": 3}]}),
        ]
    )
    conn = RestApiConnector(
        config={"base_url": BASE_URL, "auth_type": "BEARER"},
        secrets={"token": "tok-123"},
        sleep_fn=lambda _s: None,
    )
    try:
        batches = list(
            conn.read(
                {
                    "path": "/orders",
                    "record_path": "$.items",
                    "pagination": {"type": "PAGE_NUMBER", "page_size": 2},
                },
                "SNAPSHOT",
                None,
                None,
            )
        )
    finally:
        conn.close()

    total_rows = sum(b.batch.num_rows for b in batches)
    assert total_rows == 3


@respx.mock
def test_rest_cursor_and_link_header_pagination() -> None:
    respx.get(f"{BASE_URL}/events").mock(
        side_effect=[
            httpx.Response(200, json={"data": [{"k": "a"}], "next_cursor": "cur-2"}),
            httpx.Response(200, json={"data": [{"k": "b"}], "next_cursor": None}),
        ]
    )
    respx.get(f"{BASE_URL}/repos").mock(
        side_effect=[
            httpx.Response(
                200,
                json=[{"repo": "r1"}],
                headers={"Link": f'<{BASE_URL}/repos?after=1>; rel="next"'},
            ),
            httpx.Response(200, json=[{"repo": "r2"}]),
        ]
    )

    conn = RestApiConnector(config={"base_url": BASE_URL}, secrets={}, sleep_fn=lambda _s: None)
    try:
        cur_batches = list(
            conn.read(
                {
                    "path": "/events",
                    "record_path": "$.data",
                    "pagination": {"type": "CURSOR", "next_cursor_path": "$.next_cursor"},
                },
                "SNAPSHOT",
                None,
                None,
            )
        )
        link_batches = list(
            conn.read(
                {
                    "path": "/repos",
                    "record_path": "$",
                    "pagination": {"type": "LINK_HEADER"},
                },
                "SNAPSHOT",
                None,
                None,
            )
        )
    finally:
        conn.close()

    assert sum(b.batch.num_rows for b in cur_batches) == 2
    assert sum(b.batch.num_rows for b in link_batches) == 2


@respx.mock
def test_rest_oauth2_and_429_retry_recovery() -> None:
    token_route = respx.post(f"{BASE_URL}/oauth/token").mock(
        return_value=httpx.Response(200, json={"access_token": "oa-tok", "expires_in": 3600})
    )
    data_route = respx.get(f"{BASE_URL}/metrics").mock(
        side_effect=[
            httpx.Response(429, headers={"Retry-After": "0.01"}),
            httpx.Response(503),
            httpx.Response(200, json=[{"metric": "cpu", "val": 0.72}]),
        ]
    )

    conn = RestApiConnector(
        config={
            "base_url": BASE_URL,
            "auth_type": "OAUTH2_CLIENT_CREDENTIALS",
            "token_url": f"{BASE_URL}/oauth/token",
        },
        secrets={"client_id": "cid", "client_secret": "csec"},
        sleep_fn=lambda _s: None,
    )
    try:
        tbl = conn.preview({"path": "/metrics", "record_path": "$"}, limit=10)
    finally:
        conn.close()

    assert token_route.call_count == 1
    assert data_route.call_count == 3
    assert tbl.num_rows == 1


@respx.mock
def test_rest_redirect_to_metadata_ip_is_blocked_by_ssrf() -> None:
    respx.get(f"{BASE_URL}/evil-redirect").mock(
        return_value=httpx.Response(
            302, headers={"Location": "http://169.254.169.254/latest/meta-data/"}
        )
    )
    conn = RestApiConnector(config={"base_url": BASE_URL}, secrets={}, sleep_fn=lambda _s: None)
    try:
        with pytest.raises(OutboundBlocked):
            conn.preview({"path": "/evil-redirect"}, limit=5)
    finally:
        conn.close()
