#!/usr/bin/env python3
"""xBlueNode Python Sidecar 静态类型、SQL 注入防护与凭证安全守卫（Python >= 3.11）。

扫描范围：`axpoint-parser/**/*.py`（排除 `.venv` 与缓存目录）。
拦截规则：
1. 非空 `.py` 模块必须声明 `from __future__ import annotations`；
2. 禁止无具体错误码的裸 `# type: ignore`（必须写明 `# type: ignore[code]`）；
3. 生产代码（非 `tests/`）禁止 `eval(`、`exec(`、`pickle.loads(` 与不安全的 `yaml.load(`；
4. 禁止在 SQL 字符串插值中直接将变量拼入 `WHERE ... = '{...}'` 或 `VALUES ('{...}'` 值字面量位置；
5. 生产代码禁止在日志或标准输出中直接格式化输出敏感凭证变量（`password`、`secret`、`api_key`、`private_key`）。
"""
from __future__ import annotations

import ast
import os
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PARSER_ROOT = REPO_ROOT / "axpoint-parser"
SKIP_DIRS = {".venv", "__pycache__", ".mypy_cache", ".pytest_cache", ".ruff_cache", "node_modules"}

BARE_TYPE_IGNORE_RE = re.compile(r"#\s*type:\s*ignore(?!\s*\[)")
UNSAFE_SQL_VALUE_INTERP_RE = re.compile(
    r"""(?ix)
    \b(?:WHERE|AND|OR)\s+[A-Za-z0-9_."`]+\s*(?:=|!=|<>|LIKE|ILIKE)\s*'\s*\{[^}]+\}\s*'
    |\bVALUES\s*\([^)]*'\s*\{[^}]+\}\s*'
    """
)
SECRET_VAR_RE = re.compile(
    r"^(?:raw_)?(?:password|passwd|secret|client_secret|api_key|private_key|access_token|refresh_token)$",
    re.IGNORECASE,
)


def rel(path: Path) -> str:
    return str(path.relative_to(REPO_ROOT)) if path.is_relative_to(REPO_ROOT) else str(path)


def has_future_annotations(tree: ast.Module) -> bool:
    for node in tree.body:
        if (
            isinstance(node, ast.ImportFrom)
            and node.module == "__future__"
            and any(alias.name == "annotations" for alias in node.names)
        ):
            return True
    return False


def check_ast_rules(path: Path, tree: ast.Module, is_test: bool, errors: list[str]) -> None:
    if not has_future_annotations(tree):
        errors.append(f"{rel(path)}:1: 缺少 `from __future__ import annotations`")

    for node in ast.walk(tree):
        if isinstance(node, ast.JoinedStr):
            reconstructed: list[str] = []
            for part in node.values:
                if isinstance(part, ast.Constant) and isinstance(part.value, str):
                    reconstructed.append(part.value)
                else:
                    reconstructed.append("{expr}")
            joined_preview = "".join(reconstructed)
            if UNSAFE_SQL_VALUE_INTERP_RE.search(joined_preview):
                errors.append(
                    f"{rel(path)}:{node.lineno}: 禁止在 f-string SQL 条件/VALUES 中直接插值单引号字面量，请使用参数化绑定"
                )

        if is_test or not isinstance(node, ast.Call):
            continue

        func = node.func
        if isinstance(func, ast.Name) and func.id in {"eval", "exec"}:
            errors.append(f"{rel(path)}:{node.lineno}: 生产代码禁止调用 `{func.id}()`")
        elif isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name):
            pair = (func.value.id, func.attr)
            if pair == ("pickle", "loads") or pair == ("pickle", "load"):
                errors.append(f"{rel(path)}:{node.lineno}: 生产代码禁止调用 `pickle.{func.attr}()`")
            elif pair == ("yaml", "load"):
                has_safe_loader = any(
                    kw.arg == "Loader"
                    and isinstance(kw.value, ast.Attribute)
                    and "SafeLoader" in kw.value.attr
                    for kw in node.keywords
                )
                if not has_safe_loader:
                    errors.append(
                        f"{rel(path)}:{node.lineno}: 禁止无 SafeLoader 的 `yaml.load()`，请使用 `yaml.safe_load()`"
                    )

        is_log_or_print = (isinstance(func, ast.Name) and func.id == "print") or (
            isinstance(func, ast.Attribute)
            and func.attr in {"debug", "info", "warning", "warn", "error", "exception", "critical"}
        )
        if is_log_or_print:
            for arg in node.args:
                for sub in ast.walk(arg):
                    if isinstance(sub, ast.Name) and SECRET_VAR_RE.match(sub.id):
                        errors.append(
                            f"{rel(path)}:{node.lineno}: 禁止在日志/输出中直接引用敏感变量 `{sub.id}`"
                        )
                    elif isinstance(sub, ast.Attribute) and SECRET_VAR_RE.match(sub.attr):
                        errors.append(
                            f"{rel(path)}:{node.lineno}: 禁止在日志/输出中直接引用敏感属性 `.{sub.attr}`"
                        )


def scan_py_file(path: Path, errors: list[str]) -> None:
    source = path.read_text(encoding="utf-8")
    if not source.strip():
        return

    for lineno, line in enumerate(source.splitlines(), 1):
        if BARE_TYPE_IGNORE_RE.search(line):
            errors.append(
                f"{rel(path)}:{lineno}: 禁止裸 `# type: ignore`，必须显式指定错误码 `# type: ignore[code]`"
            )

    try:
        tree = ast.parse(source, filename=str(path))
    except SyntaxError as exc:
        errors.append(f"{rel(path)}:{exc.lineno or 1}: Python 语法解析失败: {exc.msg}")
        return

    rel_parts = path.relative_to(PARSER_ROOT).parts
    is_test = "tests" in rel_parts
    check_ast_rules(path, tree, is_test, errors)


def main() -> int:
    errors: list[str] = []
    if not PARSER_ROOT.is_dir():
        print(f"python-guard FAILED: 缺少目录 {rel(PARSER_ROOT)}", file=sys.stderr)
        return 1

    for cur_dir, dirs, files in os.walk(PARSER_ROOT):
        dirs[:] = [d for d in sorted(dirs) if d not in SKIP_DIRS]
        for fname in sorted(files):
            if fname.endswith(".py"):
                scan_py_file(Path(cur_dir) / fname, errors)

    if errors:
        print("python-guard FAILED:", file=sys.stderr)
        for item in errors:
            print(f"  - {item}", file=sys.stderr)
        return 1

    print("python-guard: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
