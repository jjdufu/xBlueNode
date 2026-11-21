#!/usr/bin/env python3
"""Bidirectional route consistency checker between Rust routers and A4-API-CONTRACT-INDEX.md."""

from __future__ import annotations

import argparse
import re
from pathlib import Path

_PARAM_RE = re.compile(r"\{[^/{}]+\}|:[A-Za-z_][A-Za-z0-9_]*")
_METHOD_CALL_RE = re.compile(
    r"(?:axum::routing::|\.\s*)?(get|post|put|patch|delete|head|options)\s*\("
)
_ROUTER_OP_RE = re.compile(r"\.\s*(route|nest|merge)\s*\(")
_FN_ROUTER_RE = re.compile(r"\bfn\s+([A-Za-z0-9_]+)\s*\([^)]*\)\s*->\s*Router\b[^{]*\{")
_CFG_TEST_RE = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")
_STR_TOKEN_RE = re.compile(r"__STR_(\d+)__")
_SECTION_HEADING_RE = re.compile(r"^##\s+(\d+)\.\s+(.*)$")
_PREFIX_BACKTICK_RE = re.compile(r"`(/api/[a-z0-9_/-]+)`")
_A4_ROUTE_RE = re.compile(r"`(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\s+(/[^`\s?]*)[^`]*`")

RouteEntry = tuple[str, str]
FnKey = tuple[str, str]


def normalize_path(path: str) -> str:
    """Normalize query suffix, trailing slash, and path parameters to `{param}`."""
    clean = path.split("?", 1)[0].strip()
    if len(clean) > 1 and clean.endswith("/"):
        clean = clean[:-1]
    return _PARAM_RE.sub("{param}", clean)


def join_paths(prefix: str, path: str) -> str:
    """Join an Axum `.nest(prefix, ...)` prefix with a child route path."""
    left = prefix.rstrip("/")
    right = path if path.startswith("/") else f"/{path}"
    if right == "/" and left:
        return left
    joined = f"{left}{right}" if left else right
    return joined if joined else "/"


def strip_comments_extract_strings(source: str) -> tuple[str, list[str]]:
    """Strip Rust comments and replace string literals with `__STR_<idx>__` placeholders."""
    out: list[str] = []
    strings: list[str] = []
    idx = 0
    length = len(source)

    while idx < length:
        if source.startswith("//", idx):
            nl = source.find("\n", idx)
            idx = length if nl == -1 else nl
            continue
        if source.startswith("/*", idx):
            depth = 1
            idx += 2
            while idx < length and depth > 0:
                if source.startswith("/*", idx):
                    depth += 1
                    idx += 2
                elif source.startswith("*/", idx):
                    depth -= 1
                    idx += 2
                else:
                    idx += 1
            out.append(" ")
            continue
        if source[idx] == "r" and idx + 1 < length and source[idx + 1] in ('#', '"'):
            scan = idx + 1
            hashes = 0
            while scan < length and source[scan] == "#":
                hashes += 1
                scan += 1
            if scan < length and source[scan] == '"':
                closing = '"' + ("#" * hashes)
                end_pos = source.find(closing, scan + 1)
                if end_pos != -1:
                    token_id = len(strings)
                    strings.append(source[scan + 1 : end_pos])
                    out.append(f"__STR_{token_id}__")
                    idx = end_pos + len(closing)
                    continue
        if source[idx] == '"':
            scan = idx + 1
            chars: list[str] = []
            while scan < length and source[scan] != '"':
                if source[scan] == "\\" and scan + 1 < length:
                    chars.append(source[scan + 1])
                    scan += 2
                else:
                    chars.append(source[scan])
                    scan += 1
            token_id = len(strings)
            strings.append("".join(chars))
            out.append(f"__STR_{token_id}__")
            idx = scan + 1
            continue
        if source[idx] == "'":
            if idx + 2 < length and source[idx + 1] != "\\" and source[idx + 2] == "'":
                out.append("   ")
                idx += 3
                continue
            if idx + 3 < length and source[idx + 1] == "\\" and source[idx + 3] == "'":
                out.append("    ")
                idx += 4
                continue
        out.append(source[idx])
        idx += 1

    return "".join(out), strings


def find_matching_delim(text: str, open_pos: int, open_ch: str, close_ch: str) -> int:
    """Return index of matching closing delimiter starting from `open_pos`."""
    depth = 1
    pos = open_pos + 1
    while pos < len(text) and depth > 0:
        if text[pos] == open_ch:
            depth += 1
        elif text[pos] == close_ch:
            depth -= 1
        pos += 1
    return pos - 1


def split_top_level_commas(arg_text: str) -> list[str]:
    """Split function call arguments by top-level commas."""
    parts: list[str] = []
    depth = 0
    start = 0
    for idx, ch in enumerate(arg_text):
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        elif ch == "," and depth == 0:
            parts.append(arg_text[start:idx].strip())
            start = idx + 1
    tail = arg_text[start:].strip()
    if tail:
        parts.append(tail)
    return parts


def extract_http_methods(method_expr: str) -> list[str]:
    """Extract Axum HTTP method verbs (`get(...)`, `.post(...)`, etc.) at top paren depth."""
    methods: list[str] = []
    pairs = {"(": ")", "[": "]", "{": "}"}
    pos = 0
    length = len(method_expr)
    while pos < length:
        ch = method_expr[pos]
        if ch in pairs:
            pos = find_matching_delim(method_expr, pos, ch, pairs[ch]) + 1
            continue
        match = _METHOD_CALL_RE.match(method_expr[pos:])
        if match:
            verb = match.group(1).upper()
            if verb not in methods:
                methods.append(verb)
            open_paren = pos + match.end() - 1
            pos = find_matching_delim(method_expr, open_paren, "(", ")") + 1
            continue
        pos += 1
    return methods


def strip_cfg_test_blocks(cleaned: str) -> str:
    """Remove `#[cfg(test)] { ... }` blocks so unit tests do not pollute production routes."""
    result = cleaned
    while True:
        match = _CFG_TEST_RE.search(result)
        if not match:
            return result
        brace_pos = result.find("{", match.end())
        semi_pos = result.find(";", match.end())
        if semi_pos != -1 and (brace_pos == -1 or semi_pos < brace_pos):
            result = result[: match.start()] + result[semi_pos + 1 :]
            continue
        if brace_pos == -1:
            return result[: match.start()]
        end_brace = find_matching_delim(result, brace_pos, "{", "}")
        result = result[: match.start()] + result[end_brace + 1 :]


class RustRouterExtractor:
    """Evaluates Axum `Router` builder chains across `src/routes/**/*.rs`."""

    def __init__(self, routes_dir: Path) -> None:
        self.routes_dir = routes_dir
        self.fn_defs: dict[FnKey, tuple[str, list[str]]] = {}
        self.memo: dict[FnKey, list[RouteEntry]] = {}
        self._load_files()

    def _load_files(self) -> None:
        for rs_file in sorted(self.routes_dir.rglob("*.rs")):
            rel_parts = list(rs_file.relative_to(self.routes_dir).parts)
            if rel_parts[-1] == "mod.rs":
                mod_key = "::".join(rel_parts[:-1])
            else:
                rel_parts[-1] = rel_parts[-1][:-3]
                mod_key = "::".join(rel_parts)

            raw_src = rs_file.read_text(encoding="utf-8")
            cleaned, strings = strip_comments_extract_strings(raw_src)
            cleaned = strip_cfg_test_blocks(cleaned)

            for match in _FN_ROUTER_RE.finditer(cleaned):
                fn_name = match.group(1)
                open_brace = match.end() - 1
                close_brace = find_matching_delim(cleaned, open_brace, "{", "}")
                body = cleaned[open_brace + 1 : close_brace]
                self.fn_defs[(mod_key, fn_name)] = (body, strings)

    def extract_all(self) -> set[RouteEntry]:
        if ("", "api_router") in self.fn_defs:
            entries = self._eval_fn(("", "api_router"))
        elif ("", "router") in self.fn_defs:
            entries = self._eval_fn(("", "router"))
        else:
            entries = []
            for key in sorted(self.fn_defs):
                if key[1] == "router":
                    entries.extend(self._eval_fn(key))

        spa_prefix = self._detect_spa_api_prefix()
        normalized: set[RouteEntry] = set()
        for method, raw_path in entries:
            full_path = (
                join_paths(spa_prefix, raw_path)
                if spa_prefix and not raw_path.startswith("/api")
                else raw_path
            )
            normalized.add((method, normalize_path(full_path)))
        return normalized

    def _detect_spa_api_prefix(self) -> str:
        spa_file = self.routes_dir.parent / "spa.rs"
        if not spa_file.is_file():
            return ""
        cleaned, strings = strip_comments_extract_strings(spa_file.read_text(encoding="utf-8"))
        for match in re.finditer(r"\.\s*nest\s*\(\s*__STR_(\d+)__\s*,\s*routes::", cleaned):
            return strings[int(match.group(1))]
        return ""

    def _resolve_call_target(self, cur_mod: str, expr: str) -> FnKey | None:
        match = re.match(r"^\s*([A-Za-z0-9_:]+)\s*\(", expr)
        if not match:
            return None
        segs = match.group(1).split("::")
        if segs[0] == "Router":
            return None
        fn_name = segs[-1]
        mod_segs = segs[:-1]
        cur_segs = cur_mod.split("::") if cur_mod else []
        for seg in mod_segs:
            if seg in ("crate", "routes"):
                cur_segs = []
            elif seg == "self":
                continue
            elif seg == "super":
                if cur_segs:
                    cur_segs.pop()
            else:
                cur_segs.append(seg)
        return ("::".join(cur_segs), fn_name)

    def _eval_fn(self, key: FnKey) -> list[RouteEntry]:
        if key in self.memo:
            return self.memo[key]
        self.memo[key] = []
        body, strings = self.fn_defs[key]
        resolved = self._eval_block(key[0], body, strings, {})
        self.memo[key] = resolved
        return resolved

    def _eval_block(
        self,
        cur_mod: str,
        body: str,
        strings: list[str],
        local_vars: dict[str, list[RouteEntry]],
    ) -> list[RouteEntry]:
        stmts: list[str] = []
        depth = 0
        start = 0
        pos = 0
        length = len(body)
        while pos < length:
            ch = body[pos]
            if ch in "([":
                depth += 1
            elif ch in ")]":
                depth -= 1
            elif ch == "{":
                if depth == 0:
                    end_b = find_matching_delim(body, pos, "{", "}")
                    # Keep chained `else { ... }` attached to `if` statement/expression
                    lookahead = end_b + 1
                    while True:
                        else_match = re.match(r"\s*else\b", body[lookahead:])
                        if not else_match:
                            break
                        next_b = body.find("{", lookahead + else_match.end())
                        if next_b == -1:
                            break
                        end_b = find_matching_delim(body, next_b, "{", "}")
                        lookahead = end_b + 1
                    stmts.append(body[start : end_b + 1].strip())
                    start = end_b + 1
                    pos = end_b + 1
                    continue
                depth += 1
            elif ch == "}":
                depth -= 1
            elif ch == ";" and depth == 0:
                stmts.append(body[start:pos].strip())
                start = pos + 1
            pos += 1

        tail = body[start:].strip()
        last_val: list[RouteEntry] = []

        for stmt in stmts:
            if not stmt:
                continue
            let_m = re.match(
                r"^let\s+(?:mut\s+)?([A-Za-z0-9_]+)(?:\s*:[^=]+)?\s*=\s*(.+)$",
                stmt,
                re.DOTALL,
            )
            if let_m:
                local_vars[let_m.group(1)] = self._eval_expr(
                    cur_mod, let_m.group(2), strings, local_vars
                )
                continue

            assign_m = re.match(r"^([A-Za-z0-9_]+)\s*=\s*(.+)$", stmt, re.DOTALL)
            if assign_m and assign_m.group(1) in local_vars:
                local_vars[assign_m.group(1)] = self._eval_expr(
                    cur_mod, assign_m.group(2), strings, local_vars
                )
                continue

            if stmt.startswith("if ") or stmt.startswith("if("):
                last_val = self._eval_if_chain(cur_mod, stmt, strings, local_vars)
                continue

            ret_m = re.match(r"^return\s+(.+)$", stmt, re.DOTALL)
            if ret_m:
                last_val = self._eval_expr(cur_mod, ret_m.group(1), strings, local_vars)

        if tail:
            ret_m = re.match(r"^return\s+(.+)$", tail, re.DOTALL)
            expr_str = ret_m.group(1) if ret_m else tail
            last_val = self._eval_expr(cur_mod, expr_str, strings, local_vars)

        return last_val

    def _eval_if_chain(
        self,
        cur_mod: str,
        if_text: str,
        strings: list[str],
        local_vars: dict[str, list[RouteEntry]],
    ) -> list[RouteEntry]:
        combined: list[RouteEntry] = []
        pos = 0
        while pos < len(if_text):
            brace = if_text.find("{", pos)
            if brace == -1:
                break
            end_brace = find_matching_delim(if_text, brace, "{", "}")
            branch_routes = self._eval_block(
                cur_mod, if_text[brace + 1 : end_brace], strings, local_vars
            )
            for item in branch_routes:
                if item not in combined:
                    combined.append(item)
            pos = end_brace + 1
        return combined

    def _eval_expr(
        self,
        cur_mod: str,
        expr: str,
        strings: list[str],
        local_vars: dict[str, list[RouteEntry]],
    ) -> list[RouteEntry]:
        stripped = expr.strip()
        if stripped.startswith("if ") or stripped.startswith("if("):
            return self._eval_if_chain(cur_mod, stripped, strings, local_vars)

        routes: list[RouteEntry] = []
        pairs = {"(": ")", "[": "]", "{": "}"}
        first_dot = -1
        pos = 0
        while pos < len(stripped):
            ch = stripped[pos]
            if ch in pairs:
                pos = find_matching_delim(stripped, pos, ch, pairs[ch]) + 1
                continue
            if ch == "." and not stripped.startswith("..", pos):
                first_dot = pos
                break
            pos += 1

        head = stripped[:first_dot].strip() if first_dot != -1 else stripped
        if head in local_vars:
            routes.extend(local_vars[head])
        else:
            target = self._resolve_call_target(cur_mod, head)
            if target and target in self.fn_defs:
                routes.extend(self._eval_fn(target))

        if first_dot == -1:
            return routes

        pos = first_dot
        while pos < len(stripped):
            ch = stripped[pos]
            if ch in pairs:
                pos = find_matching_delim(stripped, pos, ch, pairs[ch]) + 1
                continue
            op_match = _ROUTER_OP_RE.match(stripped[pos:])
            if op_match:
                op = op_match.group(1)
                open_p = pos + op_match.end() - 1
                close_p = find_matching_delim(stripped, open_p, "(", ")")
                args = split_top_level_commas(stripped[open_p + 1 : close_p])
                if op == "route" and len(args) >= 2:
                    str_m = _STR_TOKEN_RE.search(args[0])
                    if str_m:
                        route_path = strings[int(str_m.group(1))]
                        for verb in extract_http_methods(args[1]):
                            routes.append((verb, route_path))
                elif op == "merge" and len(args) >= 1:
                    routes.extend(self._eval_expr(cur_mod, args[0], strings, local_vars))
                elif op == "nest" and len(args) >= 2:
                    str_m = _STR_TOKEN_RE.search(args[0])
                    if str_m:
                        nest_prefix = strings[int(str_m.group(1))]
                        for verb, child_path in self._eval_expr(
                            cur_mod, args[1], strings, local_vars
                        ):
                            routes.append((verb, join_paths(nest_prefix, child_path)))
                pos = close_p + 1
                continue
            pos += 1

        return routes


def extract_a4_core_routes(index_path: Path) -> set[RouteEntry]:
    """Extract all Core HTTP `(method, normalized_path)` entries from Sections 1..11 of A4."""
    routes: set[RouteEntry] = set()
    section_num = 0
    section_prefix = ""

    for raw_line in index_path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        sec_match = _SECTION_HEADING_RE.match(line)
        if sec_match:
            section_num = int(sec_match.group(1))
            prefixes = _PREFIX_BACKTICK_RE.findall(sec_match.group(2))
            section_prefix = prefixes[0].rstrip("/") if len(prefixes) == 1 else ""
            continue

        if section_num < 1 or section_num > 11 or not line.startswith("|"):
            continue

        cols = [col.strip() for col in line.strip("|").split("|")]
        if not cols or set(cols[0]) <= {":", "-", " "}:
            continue

        first_col = cols[0]
        if "AXPOINT_METRICS_ADDR" in first_col:
            continue

        for route_match in _A4_ROUTE_RE.finditer(first_col):
            method = route_match.group(1).upper()
            raw_path = route_match.group(2)
            full_path = (
                raw_path
                if (raw_path == "/api" or raw_path.startswith("/api/"))
                else f"{section_prefix}{raw_path}"
            )
            routes.add((method, normalize_path(full_path)))

    return routes


def compare_routes(routes_dir: Path, index_path: Path) -> list[str]:
    """Return sorted bidirectional mismatch messages (empty list when 100% consistent)."""
    if not routes_dir.is_dir():
        return [f"missing routes directory: {routes_dir}"]
    if not index_path.is_file():
        return [f"missing A4 contract index file: {index_path}"]

    rust_routes = RustRouterExtractor(routes_dir).extract_all()
    a4_routes = extract_a4_core_routes(index_path)

    discrepancies: list[str] = []
    for method, path in sorted(a4_routes - rust_routes, key=lambda item: (item[1], item[0])):
        discrepancies.append(f"MISSING_IN_RUST: {method} {path}")
    for method, path in sorted(rust_routes - a4_routes, key=lambda item: (item[1], item[0])):
        discrepancies.append(f"MISSING_IN_A4: {method} {path}")
    return discrepancies


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Bidirectional consistency check between Rust routes and A4 contract index."
    )
    parser.add_argument(
        "--routes",
        default="axpoint-core/crates/axpoint-server/src/routes",
        help="Path to Rust routes directory",
    )
    parser.add_argument(
        "--index",
        default="docs/blueprint/A4-API-CONTRACT-INDEX.md",
        help="Path to A4-API-CONTRACT-INDEX.md",
    )
    args = parser.parse_args()

    errors = compare_routes(Path(args.routes), Path(args.index))
    if errors:
        for line in errors:
            print(line)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
