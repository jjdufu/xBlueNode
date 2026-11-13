from __future__ import annotations

import httpx
import pytest
import respx

from connectors.graphql_api import GraphqlConnector
from security import SidecarError

GQL_URL = "https://93.184.216.34/graphql"


@pytest.mark.parametrize(
    "forbidden_doc",
    [
        "mutation { deleteOrder(id: 1) { ok } }",
        "subscription { orderCreated { id } }",
    ],
)
def test_graphql_rejects_mutations_and_subscriptions_before_network(forbidden_doc: str) -> None:
    conn = GraphqlConnector(config={"endpoint": GQL_URL}, secrets={})
    try:
        with pytest.raises(SidecarError) as exc_info:
            conn.preview({"query": forbidden_doc, "record_path": "$.data"}, limit=10)
        assert exc_info.value.status_code == 400
        assert exc_info.value.error_code == "SQL_REJECTED"
    finally:
        conn.close()


@respx.mock
def test_graphql_introspection_and_relay_pagination() -> None:
    respx.post(GQL_URL).mock(
        side_effect=[
            httpx.Response(
                200,
                json={
                    "data": {
                        "__schema": {
                            "queryType": {
                                "fields": [
                                    {"name": "orders", "description": "Order list"},
                                ]
                            }
                        }
                    }
                },
            ),
            httpx.Response(
                200,
                json={
                    "data": {
                        "orders": {
                            "nodes": [{"id": "o1"}, {"id": "o2"}],
                            "pageInfo": {"hasNextPage": True, "endCursor": "c2"},
                        }
                    }
                },
            ),
            httpx.Response(
                200,
                json={
                    "data": {
                        "orders": {
                            "nodes": [{"id": "o3"}],
                            "pageInfo": {"hasNextPage": False, "endCursor": None},
                        }
                    }
                },
            ),
        ]
    )

    conn = GraphqlConnector(config={"endpoint": GQL_URL}, secrets={})
    try:
        disc = conn.discover()
        assert len(disc) == 1
        assert disc[0].display_name == "orders"

        batches = list(
            conn.read(
                {
                    "query": (
                        "query($after: String) { orders(after: $after) "
                        "{ nodes { id } pageInfo { hasNextPage endCursor } } }"
                    ),
                    "record_path": "$.data.orders.nodes",
                    "after_var": "after",
                    "page_info_path": "$.data.orders.pageInfo",
                },
                "SNAPSHOT",
                None,
                None,
            )
        )
        assert sum(b.batch.num_rows for b in batches) == 3
    finally:
        conn.close()
