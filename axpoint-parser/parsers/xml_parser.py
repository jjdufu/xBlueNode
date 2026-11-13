from __future__ import annotations

import io
from collections import Counter
from pathlib import Path
from typing import Any

from lxml import etree

from parsers.base import ParseContext, ParseResult
from parsers.blocks import normalize_column_names, render_markdown_table
from parsers.json_parser import flatten_json_records
from parsers.staging import write_rows_to_staging
from schemas import ExtractedTable
from security import SidecarError, check_xml_safe, parse_xml_bytes_safe

_STREAM_XML_THRESHOLD = 64 * 1024 * 1024


def _split_qname(tag: str) -> tuple[str | None, str]:
    if tag.startswith("{"):
        end = tag.find("}")
        if end != -1:
            return tag[1:end], tag[end + 1 :]
    return None, tag


def _format_tag(
    elem: etree._Element, uri_to_prefix: dict[str, str], colliding_locals: set[str]
) -> str:
    uri, local = _split_qname(str(elem.tag))
    if uri and local in colliding_locals:
        pfx = uri_to_prefix.get(uri, "")
        if pfx:
            return f"{pfx}_{local}"
    return local


def _elem_to_obj(
    elem: etree._Element,
    uri_to_prefix: dict[str, str],
    colliding_locals: set[str],
) -> Any:
    """Convert an XML element subtree into scalar or nested dict (@attr, #text, child keys)."""
    children = [c for c in elem if isinstance(c.tag, str)]
    attribs: dict[str, str] = {}
    for attr_k, attr_v in elem.attrib.items():
        a_uri, a_local = _split_qname(str(attr_k))
        if a_uri and a_local in colliding_locals:
            pfx = uri_to_prefix.get(a_uri, "")
            key = f"@{pfx}_{a_local}" if pfx else f"@{a_local}"
        else:
            key = f"@{a_local}"
        attribs[key] = str(attr_v)

    direct_text = (elem.text or "").strip()
    if not attribs and not children:
        return direct_text if direct_text != "" else None

    result: dict[str, Any] = dict(attribs)
    if direct_text:
        result["#text"] = direct_text

    grouped_children: dict[str, list[Any]] = {}
    for ch in children:
        ch_name = _format_tag(ch, uri_to_prefix, colliding_locals)
        ch_val = _elem_to_obj(ch, uri_to_prefix, colliding_locals)
        grouped_children.setdefault(ch_name, []).append(ch_val)

    for ch_name, vals in grouped_children.items():
        result[ch_name] = vals[0] if len(vals) == 1 else vals

    return result


def _render_xml_tree_markdown(
    root: etree._Element,
    uri_to_prefix: dict[str, str],
    colliding_locals: set[str],
    *,
    max_nodes: int = 200,
) -> str:
    lines: list[str] = []
    count = 0

    def _walk(el: etree._Element, depth: int) -> None:
        nonlocal count
        if count >= max_nodes:
            return
        count += 1
        tag = _format_tag(el, uri_to_prefix, colliding_locals)
        attr_s = " ".join(
            f"{_split_qname(str(k))[1]}={v!r}" for k, v in list(el.attrib.items())[:5]
        )
        txt = (el.text or "").strip()
        label = f"`<{tag}>`"
        if attr_s:
            label += f" ({attr_s})"
        if txt:
            short_txt = txt[:80] + ("..." if len(txt) > 80 else "")
            label += f": {short_txt}"
        lines.append(f"{'  ' * depth}- {label}")
        for ch in el:
            if isinstance(ch.tag, str):
                _walk(ch, depth + 1)
                if count >= max_nodes:
                    break

    _walk(root, 0)
    return "\n".join(lines)


class XmlParser:
    """Hardened XML parser with namespace stripping and record path detection (§01.10)."""

    suffixes: tuple[str, ...] = (".xml",)

    def parse(self, data: bytes, ctx: ParseContext) -> ParseResult:
        stem = Path(ctx.filename).stem or "xml_data"

        if len(data) > _STREAM_XML_THRESHOLD:
            return self._parse_large_stream(data, stem, ctx)

        root = parse_xml_bytes_safe(data)

        # Collect namespace prefix map & detect local-name collisions across distinct URIs
        uri_to_prefix: dict[str, str] = {}
        local_to_uris: dict[str, set[str]] = {}

        for el in root.iter():
            if not isinstance(el.tag, str):
                continue
            for pfx, uri in (el.nsmap or {}).items():
                if uri and uri not in uri_to_prefix:
                    uri_to_prefix[uri] = pfx or "ns"
            u, loc = _split_qname(str(el.tag))
            if u:
                local_to_uris.setdefault(loc, set()).add(u)

        colliding_locals = {loc for loc, uris in local_to_uris.items() if len(uris) > 1}

        explicit_tag = ctx.options.get("record_tag", "").strip()
        path_counts: Counter[tuple[str, ...]] = Counter()
        path_richness: dict[tuple[str, ...], int] = {}
        path_elements: dict[tuple[str, ...], list[etree._Element]] = {}

        def _scan_paths(el: etree._Element, cur_path: tuple[str, ...]) -> None:
            t_name = _format_tag(el, uri_to_prefix, colliding_locals)
            p = (*cur_path, t_name)
            path_counts[p] += 1
            richness = len(el.attrib) + sum(1 for c in el if isinstance(c.tag, str))
            if richness > path_richness.get(p, 0):
                path_richness[p] = richness
            path_elements.setdefault(p, []).append(el)
            for ch in el:
                if isinstance(ch.tag, str):
                    _scan_paths(ch, p)

        _scan_paths(root, ())

        chosen_path: tuple[str, ...] | None = None
        if explicit_tag:
            candidates = [p for p in path_counts if p[-1] == explicit_tag]
            if candidates:
                chosen_path = min(candidates, key=lambda p: (len(p), -path_counts[p]))

        if chosen_path is None:
            rich_candidates = [
                p for p, cnt in path_counts.items() if cnt >= 2 and path_richness.get(p, 0) >= 2
            ]
            if rich_candidates:
                chosen_path = min(rich_candidates, key=lambda p: (len(p), -path_counts[p]))
            else:
                rep_candidates = [p for p, cnt in path_counts.items() if cnt >= 2]
                if rep_candidates:
                    chosen_path = min(rep_candidates, key=lambda p: (len(p), -path_counts[p]))
                else:
                    chosen_path = (_format_tag(root, uri_to_prefix, colliding_locals),)

        rec_elems = path_elements.get(chosen_path, [root])
        records: list[dict[str, Any]] = []
        for el in rec_elems:
            converted = _elem_to_obj(el, uri_to_prefix, colliding_locals)
            if isinstance(converted, dict):
                records.append(converted)
            else:
                records.append({"#text": converted})

        record_tag_name = chosen_path[-1]
        flat = flatten_json_records(records, json_explode=ctx.options.get("json_explode", "AUTO"))
        if flat.depth_limit_hit:
            ctx.add_warning("NESTED_DEPTH_LIMIT", "XML element nesting exceeded depth 6")

        norm_cols, comments, renamed = normalize_column_names(flat.main_columns)
        for orig, new in renamed:
            ctx.add_warning("COLUMN_RENAMED", f"column {orig!r} normalized to {new!r}")

        tables: list[ExtractedTable] = []
        if norm_cols:
            tbl = write_rows_to_staging(
                ctx,
                stem,
                norm_cols,
                flat.main_rows,
                origin=f"record_tag={record_tag_name}",
                source_comments=comments,
                already_normalized_cols=True,
            )
            tables.append(tbl)

        md_parts: list[str] = [f"# {stem}"]
        if uri_to_prefix:
            ns_lines = "\n".join(
                f"- `{pfx}`: `{uri}`"
                for uri, pfx in sorted(uri_to_prefix.items(), key=lambda x: x[1])
            )
            md_parts.append(f"## 命名空间映射\n\n{ns_lines}")

        md_parts.append(
            "## 节点结构预览\n\n"
            + _render_xml_tree_markdown(root, uri_to_prefix, colliding_locals, max_nodes=200)
        )
        if norm_cols:
            md_parts.append(
                f"## 提取记录 `{record_tag_name}`（{len(flat.main_rows)} 行）\n\n"
                + render_markdown_table([norm_cols, *flat.main_rows], max_rows=20)
            )

        return ParseResult(
            title=stem,
            source_format="xml",
            engine="defusedxml+lxml",
            markdown="\n\n".join(md_parts),
            tables=tables,
        )

    def _parse_large_stream(self, data: bytes, stem: str, ctx: ParseContext) -> ParseResult:
        check_xml_safe(data)
        target_tag = ctx.options.get("record_tag", "").strip()
        records: list[dict[str, Any]] = []
        try:
            context = etree.iterparse(
                io.BytesIO(data),
                events=("end",),
                resolve_entities=False,
                no_network=True,
                load_dtd=False,
                huge_tree=False,
            )
            root_seen = False
            for _ev, elem in context:
                if not isinstance(elem.tag, str):
                    continue
                _, loc = _split_qname(str(elem.tag))
                if not target_tag and not root_seen:
                    # First completed leaf-parent record tag heuristic
                    if len(elem.attrib) + len(elem) >= 2:
                        target_tag = loc
                        root_seen = True
                if target_tag and loc == target_tag:
                    obj = _elem_to_obj(elem, {}, set())
                    records.append(obj if isinstance(obj, dict) else {"#text": obj})
                    elem.clear()
        except etree.XMLSyntaxError as exc:
            raise SidecarError(422, "PARSE_MALFORMED", f"XML stream error: {exc}") from exc

        flat = flatten_json_records(records)
        norm_cols, comments, _ = normalize_column_names(flat.main_columns)
        tables: list[ExtractedTable] = []
        if norm_cols:
            tables.append(
                write_rows_to_staging(
                    ctx,
                    stem,
                    norm_cols,
                    flat.main_rows,
                    origin=f"record_tag={target_tag or 'record'}",
                    source_comments=comments,
                    already_normalized_cols=True,
                )
            )
        return ParseResult(
            title=stem,
            source_format="xml",
            engine="lxml-iterparse",
            markdown=f"# {stem}\n\n流式提取 {len(records)} 条 `{target_tag}` 记录。",
            tables=tables,
        )
