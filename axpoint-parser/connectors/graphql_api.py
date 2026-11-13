from __future__ import annotations

from collections.abc import Iterator
from typing import Any

import pyarrow as pa
from graphql import OperationDefinitionNode, OperationType
from graphql import parse as parse_graphql_ast
from jsonpath_ng import parse as parse_jsonpath  # type: ignore[attr-defined]

from connectors.base import ReadBatch, format_cursor_value, records_to_arrow_table
from connectors.rest_api import RestApiConnector
from connectors.webhook import extract_records_by_jsonpath
from parsers.json_parser import flatten_json_records
from schemas import ConnectorSpecModel, DiscoveredObject
from security import SidecarError, check_outbound_url

GRAPHQL_SPEC = ConnectorSpecModel(
    connector_type="GRAPHQL",
    display_name="GraphQL API",
    category="API",
    config_schema={
        "type": "object",
        "required": ["endpoint"],
        "properties": {
            "endpoint": {"type": "string"},
            "auth_type": {
                "type": "string",
                "enum": [
                    "NONE",
                    "BEARER",
                    "API_KEY_HEADER",
                    "API_KEY_QUERY",
                    "BASIC",
                    "OAUTH2_CLIENT_CREDENTIALS",
                ],
                "default": "NONE",
            },
            "api_key_name": {"type": "string", "default": "X-API-Key"},
            "token_url": {"type": "string"},
            "headers": {"type": "object"},
            "timeout_s": {"type": "number", "default": 30.0},
            "max_pages": {"type": "integer", "default": 500},
        },
    },
    secret_fields=["username", "password", "token", "api_key", "client_id", "client_secret"],
    supports_modes=["SNAPSHOT", "APPEND"],
    supports_discover=True,
    image_tier="lite",
)

_INTROSPECTION_QUERY = """
query AxHubIntrospection {
  __schema {
    queryType {
      fields {
        name
        description
      }
    }
  }
}
"""


def validate_graphql_query_readonly(query_str: str) -> str:
    """Parse GraphQL document via graphql-core and reject mutation/subscription operations."""
    cleaned = query_str.strip()
    if not cleaned:
        raise SidecarError(400, "SQL_REJECTED", "GraphQL query is empty")
    try:
        doc = parse_graphql_ast(cleaned)
    except Exception as exc:
        raise SidecarError(400, "SQL_REJECTED", f"invalid GraphQL syntax: {exc}") from exc

    has_query = False
    for defn in doc.definitions:
        if isinstance(defn, OperationDefinitionNode):
            if defn.operation != OperationType.QUERY:
                raise SidecarError(
                    400,
                    "SQL_REJECTED",
                    f"GraphQL {defn.operation.value} operations are forbidden",
                )
            has_query = True

    if not has_query:
        raise SidecarError(400, "SQL_REJECTED", "GraphQL document contains no query operation")
    return cleaned


class GraphqlConnector(RestApiConnector):
    """Read-only GraphQL connector enforcing query-only AST validation and Relay pagination."""

    spec = GRAPHQL_SPEC

    def __init__(
        self,
        config: dict[str, Any],
        secrets: dict[str, str],
        *,
        allow_private_network: bool = False,
        tenant_id: str = "default",
    ) -> None:
        endpoint = str(config.get("endpoint") or config.get("base_url") or "").strip()
        if not endpoint:
            raise SidecarError(400, "INVALID_ARGUMENT", "GraphQL config missing 'endpoint'")
        check_outbound_url(endpoint, allow_private_network=allow_private_network)
        merged = dict(config)
        merged["base_url"] = endpoint
        super().__init__(
            merged,
            secrets,
            allow_private_network=allow_private_network,
            tenant_id=tenant_id,
        )
        self._endpoint = endpoint

    def _exec_gql(self, query: str, variables: dict[str, Any] | None = None) -> dict[str, Any]:
        safe_q = validate_graphql_query_readonly(query)
        resp = self._send_ssrf_safe(
            "POST",
            self._endpoint,
            json_body={"query": safe_q, "variables": variables or {}},
        )
        if resp.status_code >= 400:
            raise SidecarError(
                502, "UPSTREAM_CONNECTOR", f"GraphQL endpoint returned HTTP {resp.status_code}"
            )
        payload: dict[str, Any] = resp.json()
        if payload.get("errors"):
            first_err = payload["errors"][0]
            msg = (
                first_err.get("message", "GraphQL error")
                if isinstance(first_err, dict)
                else str(first_err)
            )
            raise SidecarError(502, "UPSTREAM_CONNECTOR", f"GraphQL error: {msg}")
        return payload

    def test(self) -> tuple[bool, str]:
        try:
            self._exec_gql("query { __typename }")
            return True, "connected"
        except SidecarError as exc:
            if exc.error_code in {"PERMISSION_DENIED", "SQL_REJECTED"}:
                raise
            return False, exc.message
        except Exception as exc:
            return False, str(exc)

    def discover(self) -> list[DiscoveredObject]:
        payload = self._exec_gql(_INTROSPECTION_QUERY)
        fields = payload.get("data", {}).get("__schema", {}).get("queryType", {}).get("fields", [])
        out: list[DiscoveredObject] = []
        for f in fields:
            name = str(f.get("name") or "")
            if not name or name.startswith("__"):
                continue
            out.append(
                DiscoveredObject(
                    ref={
                        "query": f"query {{ {name} {{ __typename }} }}",
                        "record_path": f"$.data.{name}",
                    },
                    display_name=name,
                    source_kind="API_RESOURCE",
                    columns=[],
                )
            )
        return out

    def _fetch_all_records(
        self, ref: dict[str, Any], max_records: int | None = None
    ) -> list[dict[str, Any]]:
        query = str(ref.get("query") or "")
        record_path = str(ref.get("record_path") or "$.data")
        variables: dict[str, Any] = dict(ref.get("variables") or {})
        after_var = str(ref.get("after_var") or "").strip()
        page_info_path = str(ref.get("page_info_path") or "").strip()
        max_pages = min(max(int(self.config.get("max_pages") or 500), 1), 5000)

        collected: list[dict[str, Any]] = []
        after_cursor: str | None = None

        for page_i in range(max_pages):
            cur_vars = dict(variables)
            if after_var and page_i > 0:
                cur_vars[after_var] = after_cursor
            payload = self._exec_gql(query, cur_vars)
            recs = extract_records_by_jsonpath(payload, record_path)
            if recs:
                collected.extend(recs)
                if max_records is not None and len(collected) >= max_records:
                    return collected[:max_records]

            if not after_var or not page_info_path:
                break

            pi_matches = [m.value for m in parse_jsonpath(page_info_path).find(payload)]
            pi = pi_matches[0] if pi_matches and isinstance(pi_matches[0], dict) else {}
            if not pi.get("hasNextPage"):
                break
            nxt = pi.get("endCursor")
            if not nxt or str(nxt) == after_cursor:
                break
            after_cursor = str(nxt)

        return collected

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        capped = min(max(limit, 1), 1000)
        docs = self._fetch_all_records(ref, max_records=capped)
        if not docs:
            return pa.table({})
        flat = flatten_json_records(docs, json_explode="NONE")
        row_dicts = [dict(zip(flat.main_columns, r, strict=False)) for r in flat.main_rows]
        return records_to_arrow_table(row_dicts, column_order=flat.main_columns)

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        docs = self._fetch_all_records(ref)
        if mode.upper() == "APPEND" and cursor_column and cursor_from is not None:
            docs = [
                d
                for d in docs
                if d.get(cursor_column) is not None and str(d.get(cursor_column)) > cursor_from
            ]
        if not docs:
            return
        max_c: str | None = None
        if cursor_column:
            vals = [d.get(cursor_column) for d in docs if d.get(cursor_column) is not None]
            if vals:
                max_c = format_cursor_value(vals[-1])
        flat = flatten_json_records(docs, json_explode="NONE")
        row_dicts = [dict(zip(flat.main_columns, r, strict=False)) for r in flat.main_rows]
        tbl = records_to_arrow_table(row_dicts, column_order=flat.main_columns)
        for rb in tbl.to_batches(max_chunksize=max(batch_rows, 1)):
            yield ReadBatch(batch=rb, cursor_to=max_c)
