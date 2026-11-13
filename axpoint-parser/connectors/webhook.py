from __future__ import annotations

from collections.abc import Iterator
from datetime import UTC, datetime
from typing import Any

import orjson
import pyarrow as pa
from jsonpath_ng import parse as parse_jsonpath  # type: ignore[attr-defined]

from connectors.base import BaseConnector, ReadBatch, records_to_arrow_table
from parsers.json_parser import flatten_json_records
from schemas import ConnectorSpecModel

WEBHOOK_SPEC = ConnectorSpecModel(
    connector_type="WEBHOOK",
    display_name="Inbound Webhook",
    category="API",
    config_schema={
        "type": "object",
        "properties": {
            "slug": {"type": "string"},
            "auth_scheme": {
                "type": "string",
                "enum": ["HMAC_SHA256", "BEARER"],
                "default": "HMAC_SHA256",
            },
            "record_path": {"type": "string", "default": "$"},
            "max_body_bytes": {"type": "integer", "default": 1048576},
        },
    },
    secret_fields=["secret"],
    supports_modes=["APPEND"],
    supports_discover=False,
    image_tier="lite",
)


def extract_records_by_jsonpath(payload: Any, record_path: str) -> list[dict[str, Any]]:
    """Extract record dicts from JSON payload via JSONPath (unwrapping matched lists)."""
    expr_str = (record_path or "$").strip() or "$"
    if expr_str == "$":
        matches = [payload]
    else:
        expr = parse_jsonpath(expr_str)
        matches = [m.value for m in expr.find(payload)]

    records: list[dict[str, Any]] = []
    for m_val in matches:
        if isinstance(m_val, list):
            for el in m_val:
                if isinstance(el, dict):
                    records.append(dict(el))
                elif el is not None:
                    records.append({"value": el})
        elif isinstance(m_val, dict):
            records.append(dict(m_val))
        elif m_val is not None:
            records.append({"value": m_val})
    return records


class WebhookConnector(BaseConnector):
    """Connector flushing buffered inbound ds_webhook_buffer payloads into DuckLake."""

    spec = WEBHOOK_SPEC

    def test(self) -> tuple[bool, str]:
        return True, "webhook endpoint ready"

    def _collect_records(self, ref: dict[str, Any]) -> list[dict[str, Any]]:
        record_path = str(ref.get("record_path") or self.config.get("record_path") or "$")
        raw_items: list[Any] = []
        for candidate_key in ("items", "events", "buffer", "deliveries", "records"):
            val = ref.get(candidate_key)
            if isinstance(val, list):
                raw_items = val
                break
        if not raw_items and isinstance(self.config.get("items"), list):
            raw_items = list(self.config["items"])

        flattened_all: list[dict[str, Any]] = []
        for item_idx, item in enumerate(raw_items):
            if (
                isinstance(item, dict)
                and "payload" in item
                and ("delivery_id" in item or "id" in item or "received_at" in item)
            ):
                delivery_id = str(item.get("delivery_id") or item.get("id") or f"d-{item_idx}")
                raw_payload = item["payload"]
                rec_ts = item.get("received_at")
            else:
                delivery_id = f"d-{item_idx}"
                raw_payload = item
                rec_ts = None

            if isinstance(raw_payload, (bytes, bytearray)):
                raw_payload = orjson.loads(raw_payload)
            elif isinstance(raw_payload, str):
                try:
                    raw_payload = orjson.loads(raw_payload)
                except orjson.JSONDecodeError:
                    raw_payload = {"raw": raw_payload}

            extracted = extract_records_by_jsonpath(raw_payload, record_path)
            if not extracted:
                continue

            flat = flatten_json_records(extracted, json_explode="NONE")
            for sub_idx, row_vals in enumerate(flat.main_rows):
                row_dict = dict(zip(flat.main_columns, row_vals, strict=False))
                row_dict["_ax_msg_id"] = f"{delivery_id}:{sub_idx}"
                if rec_ts is not None and "_ax_received_at" not in row_dict:
                    row_dict["_ax_received_at"] = rec_ts
                elif "_ax_received_at" not in row_dict:
                    row_dict["_ax_received_at"] = (
                        datetime.now(UTC).isoformat().replace("+00:00", "Z")
                    )
                flattened_all.append(row_dict)

        return flattened_all

    def preview(self, ref: dict[str, Any], limit: int) -> pa.Table:
        recs = self._collect_records(ref)[: min(max(limit, 1), 1000)]
        return records_to_arrow_table(recs)

    def read(
        self,
        ref: dict[str, Any],
        mode: str,
        cursor_column: str | None,
        cursor_from: str | None,
        batch_rows: int = 50_000,
        stream_offsets: dict[str, str] | None = None,
    ) -> Iterator[ReadBatch]:
        recs = self._collect_records(ref)
        if not recs:
            return
        tbl = records_to_arrow_table(recs)
        for rb in tbl.to_batches(max_chunksize=max(batch_rows, 1)):
            yield ReadBatch(batch=rb)
