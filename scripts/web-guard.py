#!/usr/bin/env python3
"""xBlueNode Web 前端严格类型、设计原语与微文案静态守卫（Python >= 3.11，仅依赖标准库）。

扫描范围：`xbluenode-web/tsconfig.json` 与 `xbluenode-web/src/**/*.{ts,tsx}`。
拦截规则：
1. `tsconfig.json` 必须开启 `"strict": true`；
2. 禁止 `@ts-ignore`、`@ts-expect-error`、`@ts-nocheck`、文件级 `/* eslint-disable` 与 `@typescript-eslint` 豁免；
3. 禁止显式 `any` 类型逃逸（`: any`、`as any`、`<any>`、`Array<any>`）；
4. 禁止绕过 `src/components/ui/primitives.tsx` 直接渲染原生 HTML `<select>`；
5. 禁止违反产品微文案规范的交互赘词、回调链按钮文案、冗长引导占位符与教学横幅。
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
WEB_ROOT = REPO_ROOT / "xbluenode-web"
SRC_ROOT = WEB_ROOT / "src"

COMMENT_DIRECTIVE_PATTERNS: list[tuple[str, re.Pattern[str]]] = [
    ("ts-ignore", re.compile(r"@ts-ignore\b")),
    ("ts-expect-error", re.compile(r"@ts-expect-error\b")),
    ("ts-nocheck", re.compile(r"@ts-nocheck\b")),
    ("eslint-file-disable", re.compile(r"/\*\s*eslint-disable\b")),
    ("eslint-ts-disable", re.compile(r"eslint-disable[^\n]*@typescript-eslint/")),
]

ANY_ESCAPE_PATTERNS: list[tuple[str, re.Pattern[str]]] = [
    ("colon-any", re.compile(r":\s*any\b")),
    ("as-any", re.compile(r"\bas\s+any\b")),
    ("generic-any", re.compile(r"<\s*any\s*[,>]")),
]

NATIVE_SELECT_RE = re.compile(r"<select[\s>]")

MICROCOPY_ANTI_PATTERNS: list[tuple[str, re.Pattern[str]]] = [
    (
        "ui-click-prefix",
        re.compile(
            r"(点击|点此|按下?)\s*(确定|保存|提交|取消|按钮|此处|这里|启动|关闭)"
            r"|\b(click|tap|press)\s+(here\s+to|to\s+(confirm|save|submit|open))\b",
            re.IGNORECASE,
        ),
    ),
    (
        "ui-chained-action",
        re.compile(r"(确定|保存|提交|点击).{0,4}(后|并)(保存|触发|执行|跳转|进入|开启)"),
    ),
    (
        "ui-verbose-placeholder",
        re.compile(r"请输入.{1,15}(以便|用于|来进行|以完成)|您可以在(此页面|这里|下方)"),
    ),
]


def rel(path: Path) -> str:
    return str(path.relative_to(REPO_ROOT)) if path.is_relative_to(REPO_ROOT) else str(path)


def strip_ts_comments_and_strings(source: str) -> str:
    """保留换行符并抹除注释与字符串字面量，供语法级 `any` 与 `<select>` 扫描使用。"""
    out: list[str] = []
    i = 0
    n = len(source)
    while i < n:
        ch = source[i]
        if source.startswith("//", i):
            nl = source.find("\n", i)
            if nl == -1:
                break
            out.append("\n")
            i = nl + 1
            continue
        if source.startswith("/*", i):
            end = source.find("*/", i + 2)
            if end == -1:
                break
            segment = source[i : end + 2]
            out.append("\n" * segment.count("\n"))
            i = end + 2
            continue
        if ch in ("'", '"', "`"):
            quote = ch
            out.append(quote)
            i += 1
            while i < n:
                c = source[i]
                if c == "\\" and i + 1 < n:
                    if source[i + 1] == "\n":
                        out.append("\n")
                    i += 2
                    continue
                if c == "\n":
                    out.append("\n")
                if c == quote:
                    out.append(quote)
                    i += 1
                    break
                i += 1
            continue
        out.append(ch)
        i += 1
    return "".join(out)


def check_tsconfig(errors: list[str]) -> None:
    tsconfig_path = WEB_ROOT / "tsconfig.json"
    if not tsconfig_path.is_file():
        errors.append("缺少 xbluenode-web/tsconfig.json")
        return
    raw = tsconfig_path.read_text(encoding="utf-8")
    cleaned = re.sub(r"//[^\n]*", "", raw)
    try:
        parsed = json.loads(cleaned)
    except json.JSONDecodeError as exc:
        errors.append(f"{rel(tsconfig_path)}: JSON 解析失败: {exc}")
        return
    compiler_opts = parsed.get("compilerOptions", {})
    if not isinstance(compiler_opts, dict) or compiler_opts.get("strict") is not True:
        errors.append(f"{rel(tsconfig_path)}: compilerOptions.strict 必须为 true")


def scan_source_file(path: Path, errors: list[str]) -> None:
    raw = path.read_text(encoding="utf-8")
    raw_lines = raw.splitlines()
    for lineno, line in enumerate(raw_lines, 1):
        for rule_id, pat in COMMENT_DIRECTIVE_PATTERNS:
            if pat.search(line):
                errors.append(f"{rel(path)}:{lineno}: 禁止类型/Lint 豁免指令 [{rule_id}]")
        for rule_id, pat in MICROCOPY_ANTI_PATTERNS:
            m = pat.search(line)
            if m:
                errors.append(
                    f"{rel(path)}:{lineno}: 违反产品微文案规范 [{rule_id}] 命中 {m.group(0)!r}"
                )

    stripped = strip_ts_comments_and_strings(raw)
    for lineno, line in enumerate(stripped.splitlines(), 1):
        for rule_id, pat in ANY_ESCAPE_PATTERNS:
            if pat.search(line):
                errors.append(f"{rel(path)}:{lineno}: 禁止使用显式 any 类型 [{rule_id}]")
        if path.name != "primitives.tsx" and NATIVE_SELECT_RE.search(line):
            errors.append(
                f"{rel(path)}:{lineno}: 禁止直接使用原生 <select>，请统一使用 primitives.tsx 中的 Select"
            )


def main() -> int:
    errors: list[str] = []
    check_tsconfig(errors)

    if not SRC_ROOT.is_dir():
        errors.append(f"缺少前端源码目录: {rel(SRC_ROOT)}")
    else:
        for path in sorted(SRC_ROOT.rglob("*")):
            if path.is_file() and path.suffix in {".ts", ".tsx"}:
                scan_source_file(path, errors)

    if errors:
        print("web-guard FAILED:", file=sys.stderr)
        for item in errors:
            print(f"  - {item}", file=sys.stderr)
        return 1

    print("web-guard: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
