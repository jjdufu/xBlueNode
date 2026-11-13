from __future__ import annotations

import io
from collections import deque
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from decimal import Decimal
from pathlib import Path
from typing import Any

import ijson
import orjson
from genson import SchemaBuilder

from parsers.base import ParseContext, ParseResult
from parsers.blocks import (
    normalize_column_names,
    normalize_table_name,
    render_markdown_table,
)
from parsers.staging import write_rows_to_staging
from schemas import ExtractedTable
from security import SidecarError

_STREAM_THRESHOLD_BYTES = 64 * 1024 * 1024
_MAX_FLATTEN_DEPTH = 6


@dataclass
class FlattenedJsonTables:
    """Result of flattening semi-structured JSON records with child-array explosion."""

    main_columns: list[str]
    main_rows: list[list[Any]]
    child_tables: dict[str, tuple[list[str], list[list[Any]]]] = field(default_factory=dict)
    depth_limit_hit: bool = False


def _json_default(obj: Any) -> Any:
    if isinstance(obj, Decimal):
        return float(obj)
    return str(obj)


def locate_record_array(root: Any) -> tuple[list[dict[str, Any]], str]:
    """Locate record collection inside parsed JSON per Step 01 §01.9 (BFS)."""
    if isinstance(root, list):
        records = [item if isinstance(item, dict) else {"value": item} for item in root]
        return records, "$"

    if isinstance(root, dict):
        queue: deque[tuple[str, Any]] = deque([("$", root)])
        while queue:
            cur_path, node = queue.popleft()
            if isinstance(node, dict):
                for k, v in node.items():
                    child_path = f"{cur_path}.{k}"
                    if (
                        isinstance(v, list)
                        and len(v) >= 2
                        and all(isinstance(el, dict) for el in v)
                    ):
                        return list(v), child_path
                    if isinstance(v, (dict, list)):
                        queue.append((child_path, v))
            elif isinstance(node, list):
                for idx, el in enumerate(node):
                    if isinstance(el, (dict, list)):
                        queue.append((f"{cur_path}[{idx}]", el))
        return [root], "$"

    return [{"value": root}], "$"


def flatten_json_records(
    records: Sequence[dict[str, Any]],
    *,
    json_explode: str = "AUTO",
    max_depth: int = _MAX_FLATTEN_DEPTH,
) -> FlattenedJsonTables:
    """Flatten nested objects with '.' up to depth 6 and handle array fields per json_explode."""
    mode = json_explode.upper()
    if mode not in {"AUTO", "NONE", "ALL"}:
        mode = "AUTO"

    depth_hit = False
    main_dicts: list[dict[str, Any]] = []
    main_col_order: list[str] = []
    child_dicts: dict[str, list[dict[str, Any]]] = {}
    child_col_orders: dict[str, list[str]] = {}

    def _register_col(order: list[str], key: str) -> None:
        if key not in order:
            order.append(key)

    def _flatten_obj(
        obj: dict[str, Any],
        prefix: str,
        depth: int,
        out: dict[str, Any],
        col_order: list[str],
        parent_row_idx: int | None,
    ) -> list[tuple[str, list[dict[str, Any]]]]:
        nonlocal depth_hit
        cartesian_arrays: list[tuple[str, list[dict[str, Any]]]] = []

        for k, v in obj.items():
            full_key = f"{prefix}.{k}" if prefix else str(k)
            if isinstance(v, dict):
                if not v:
                    out[full_key] = None
                    _register_col(col_order, full_key)
                elif depth >= max_depth:
                    depth_hit = True
                    out[full_key] = orjson.dumps(v, default=_json_default).decode("utf-8")
                    _register_col(col_order, full_key)
                else:
                    cartesian_arrays.extend(
                        _flatten_obj(v, full_key, depth + 1, out, col_order, parent_row_idx)
                    )
            elif isinstance(v, list):
                if mode == "NONE":
                    out[full_key] = orjson.dumps(v, default=_json_default).decode("utf-8")
                    _register_col(col_order, full_key)
                elif v and all(isinstance(el, dict) for el in v):
                    if mode == "ALL" and parent_row_idx is not None:
                        cartesian_arrays.append((full_key, list(v)))
                    elif parent_row_idx is not None:
                        # AUTO: explode object array into child table
                        c_key = full_key.replace(".", "_")
                        c_rows = child_dicts.setdefault(c_key, [])
                        c_cols = child_col_orders.setdefault(c_key, ["_parent_row"])
                        for elem in v:
                            c_flat: dict[str, Any] = {"_parent_row": parent_row_idx}
                            _flatten_obj(elem, "", 1, c_flat, c_cols, None)
                            c_rows.append(c_flat)
                    else:
                        out[full_key] = orjson.dumps(v, default=_json_default).decode("utf-8")
                        _register_col(col_order, full_key)
                elif not v:
                    # Empty list: wait to see if other rows populate child table vs scalar list
                    if full_key not in child_dicts:
                        out[full_key] = []
                        _register_col(col_order, full_key)
                else:
                    # Scalar or mixed list -> keep as list for LIST column
                    out[full_key] = v
                    _register_col(col_order, full_key)
            else:
                out[full_key] = v
                _register_col(col_order, full_key)

        return cartesian_arrays

    for row_idx, rec in enumerate(records):
        base_flat: dict[str, Any] = {}
        cart_fields = _flatten_obj(rec, "", 1, base_flat, main_col_order, row_idx)
        if not cart_fields:
            main_dicts.append(base_flat)
        else:
            # Cartesian expansion for json_explode == ALL
            expanded: list[dict[str, Any]] = [base_flat]
            for arr_key, dict_items in cart_fields:
                next_expanded: list[dict[str, Any]] = []
                for partial in expanded:
                    if not dict_items:
                        next_expanded.append(partial)
                    else:
                        for d_item in dict_items:
                            merged = dict(partial)
                            _flatten_obj(d_item, arr_key, 2, merged, main_col_order, None)
                            next_expanded.append(merged)
                expanded = next_expanded
            main_dicts.extend(expanded)

    # Remove any column from main_col_order that ended up exploded into child_dicts
    exploded_keys = set(child_dicts.keys())
    final_main_cols = [c for c in main_col_order if c.replace(".", "_") not in exploded_keys]
    main_rows = [[d.get(c) for c in final_main_cols] for d in main_dicts]

    child_tables: dict[str, tuple[list[str], list[list[Any]]]] = {}
    for c_key, c_list in child_dicts.items():
        c_cols = child_col_orders[c_key]
        c_matrix = [[cd.get(col) for col in c_cols] for cd in c_list]
        child_tables[c_key] = (c_cols, c_matrix)

    return FlattenedJsonTables(
        main_columns=final_main_cols,
        main_rows=main_rows,
        child_tables=child_tables,
        depth_limit_hit=depth_hit,
    )


def build_json_schema_dict(
    records: Iterable[dict[str, Any]], max_sample: int = 500
) -> dict[str, Any]:
    """Generate JSON Schema using genson over sampled records."""
    builder = SchemaBuilder()
    builder.add_schema({"type": "object", "properties": {}})
    for idx, rec in enumerate(records):
        if idx >= max_sample:
            break
        builder.add_object(rec)
    schema_obj: dict[str, Any] = builder.to_schema()
    return schema_obj


class JsonParser:
    """Parser for .json, .jsonl, and .ndjson files (Step 01 §01.9)."""

    suffixes: tuple[str, ...] = (".json", ".jsonl", ".ndjson")

    def parse(self, data: bytes, ctx: ParseContext) -> ParseResult:
        suffix = Path(ctx.filename).suffix.lower()
        stem = Path(ctx.filename).stem or "json_data"
        is_jsonl = suffix in {".jsonl", ".ndjson"}
        source_format = "jsonl" if is_jsonl else "json"
        explode_mode = ctx.options.get("json_explode", "AUTO").upper()

        if is_jsonl:
            records, origin = self._read_jsonl(data, ctx)
            engine = "orjson-jsonl"
        elif len(data) > _STREAM_THRESHOLD_BYTES:
            records, origin = self._read_large_ijson(data)
            engine = "ijson-stream"
        else:
            try:
                parsed = orjson.loads(data)
            except orjson.JSONDecodeError as exc:
                # Could be NDJSON uploaded with .json suffix
                lines = [ln for ln in data.splitlines() if ln.strip()]
                if len(lines) >= 2:
                    records, origin = self._read_jsonl(data, ctx)
                    source_format = "jsonl"
                    engine = "orjson-jsonl"
                else:
                    raise SidecarError(
                        422, "PARSE_MALFORMED", f"invalid JSON syntax: {exc}"
                    ) from exc
            else:
                records, rec_path = locate_record_array(parsed)
                origin = f"record_path={rec_path}"
                engine = "orjson"

        flat = flatten_json_records(records, json_explode=explode_mode)
        if flat.depth_limit_hit:
            ctx.add_warning(
                "NESTED_DEPTH_LIMIT",
                f"nested JSON object exceeded max flatten depth {_MAX_FLATTEN_DEPTH}",
            )

        base_norm_name = normalize_table_name(stem)
        norm_cols, comments, renamed = normalize_column_names(flat.main_columns)
        for orig, new in renamed:
            ctx.add_warning("COLUMN_RENAMED", f"column {orig!r} normalized to {new!r}")

        tables: list[ExtractedTable] = []
        md_sections: list[str] = [f"# {stem}"]

        if norm_cols:
            main_tbl = write_rows_to_staging(
                ctx,
                base_norm_name,
                norm_cols,
                flat.main_rows,
                origin=origin,
                source_comments=comments,
                already_normalized_cols=True,
            )
            tables.append(main_tbl)
            md_sections.append(
                f"- **主表**：`{main_tbl.table_name}`（{main_tbl.row_count} 行，`{origin}`）\n\n"
                + render_markdown_table([norm_cols, *flat.main_rows], max_rows=20)
            )

            if ctx.write_staging:
                schema_dict = build_json_schema_dict(records)
                ctx.staging_dir.mkdir(parents=True, exist_ok=True)
                schema_file = ctx.staging_dir / f"{main_tbl.table_name}.schema.json"
                schema_file.write_bytes(orjson.dumps(schema_dict, option=orjson.OPT_INDENT_2))

        for child_field, (c_cols_raw, c_rows) in flat.child_tables.items():
            c_cols, c_comments, c_renamed = normalize_column_names(c_cols_raw)
            for orig, new in c_renamed:
                ctx.add_warning("COLUMN_RENAMED", f"child column {orig!r} normalized to {new!r}")
            child_raw_name = f"{base_norm_name}__{child_field}"
            child_tbl = write_rows_to_staging(
                ctx,
                child_raw_name,
                c_cols,
                c_rows,
                origin=f"{origin};child={child_field}",
                source_comments=c_comments,
                already_normalized_cols=True,
            )
            tables.append(child_tbl)
            md_sections.append(
                f"## 子表 `{child_tbl.table_name}`（{child_tbl.row_count} 行）\n\n"
                + render_markdown_table([c_cols, *c_rows], max_rows=20)
            )

        return ParseResult(
            title=stem,
            source_format=source_format,
            engine=engine,
            markdown="\n\n".join(md_sections),
            tables=tables,
        )

    def _read_jsonl(self, data: bytes, ctx: ParseContext) -> tuple[list[dict[str, Any]], str]:
        records: list[dict[str, Any]] = []
        malformed = 0
        for raw_line in data.splitlines():
            line = raw_line.strip()
            if not line:
                continue
            try:
                obj = orjson.loads(line)
            except orjson.JSONDecodeError:
                malformed += 1
                continue
            if isinstance(obj, dict):
                records.append(obj)
            else:
                records.append({"value": obj})

        if malformed > 0:
            ctx.add_warning("ROW_MALFORMED", f"skipped {malformed} malformed JSONL lines")
        if not records and malformed > 0:
            raise SidecarError(422, "PARSE_MALFORMED", "no valid JSONL records found")
        return records, "record_path=$[*]"

    def _read_large_ijson(self, data: bytes) -> tuple[list[dict[str, Any]], str]:
        records: list[dict[str, Any]] = []
        try:
            for item in ijson.items(io.BytesIO(data), "item"):
                records.append(item if isinstance(item, dict) else {"value": item})
        except Exception as exc:
            raise SidecarError(
                422, "PARSE_MALFORMED", f"streaming JSON parse error: {exc}"
            ) from exc
        return records, "record_path=$[*]"
