#!/usr/bin/env python3
"""xBlueNode Rust 门禁静态防绕过守卫（Python >= 3.11，仅依赖标准库）。

扫描范围：`axpoint-core/` 下的工具链锁定、`Cargo.toml`、`clippy.toml`、`rustfmt.toml`、
`.cargo/config(.toml)` 以及全部 `.rs` 源码（剥离注释与字符串字面量后匹配），发现任何降级、
豁免或绕路配置立即以非零状态退出。
"""
from __future__ import annotations

import os
import re
import sys
import tomllib
from pathlib import Path
from typing import Callable, Iterator

REPO_ROOT = Path(__file__).resolve().parent.parent
RUST_ROOT = REPO_ROOT / "axpoint-core"
SKIP_DIRS = {"target", ".git", "node_modules", ".venv", "dist"}
EXPECTED_TOOLCHAIN = "1.100.0"

errs: list[str] = []


def err(msg: str) -> None:
    errs.append(msg)


def walk_rust_root(pred: Callable[[str], bool]) -> Iterator[Path]:
    for cur_dir, dirs, files in os.walk(RUST_ROOT):
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
        for fname in sorted(files):
            if pred(fname):
                yield Path(cur_dir) / fname


def load_toml(path: Path) -> dict[str, object]:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        err(f"{rel(path)}: TOML 解析失败 {exc}")
        return {}


def rel(path: Path) -> str:
    return str(path.relative_to(REPO_ROOT)) if path.is_relative_to(REPO_ROOT) else str(path)


# ---------- 1. rust-toolchain.toml 与 Cargo.lock ----------
tc_file = RUST_ROOT / "rust-toolchain.toml"
if not tc_file.is_file():
    err("缺少 axpoint-core/rust-toolchain.toml")
else:
    tc = load_toml(tc_file).get("toolchain")
    if not isinstance(tc, dict):
        err("axpoint-core/rust-toolchain.toml 缺少 [toolchain]")
    else:
        if tc.get("channel") != EXPECTED_TOOLCHAIN:
            err(f"rust-toolchain.toml channel 必须锁定为 {EXPECTED_TOOLCHAIN!r}，实际为 {tc.get('channel')!r}")
        comps = set(tc.get("components", [])) if isinstance(tc.get("components"), list) else set()
        for needed in ("clippy", "rustfmt"):
            if needed not in comps:
                err(f"rust-toolchain.toml components 缺少 {needed}")

if not (RUST_ROOT / "Cargo.lock").is_file():
    err("缺少 axpoint-core/Cargo.lock（必须提交锁文件）")

# ---------- 2. Workspace 与成员 Cargo.toml ----------
ALLOWED_LEVELS = {"deny", "forbid"}
REQUIRED_LINTS = {
    ("rust", "warnings"),
    ("rust", "unsafe_code"),
    ("rust", "missing_docs"),
    ("rustdoc", "all"),
    ("clippy", "all"),
    ("clippy", "pedantic"),
    ("clippy", "allow_attributes"),
    ("clippy", "allow_attributes_without_reason"),
    ("clippy", "unwrap_used"),
    ("clippy", "expect_used"),
    ("clippy", "panic"),
    ("clippy", "todo"),
    ("clippy", "unimplemented"),
    ("clippy", "unreachable"),
    ("clippy", "dbg_macro"),
    ("clippy", "print_stdout"),
    ("clippy", "print_stderr"),
    ("clippy", "exit"),
    ("clippy", "as_conversions"),
    ("clippy", "indexing_slicing"),
    ("clippy", "let_underscore_must_use"),
    ("clippy", "unused_result_ok"),
    ("clippy", "mem_forget"),
    ("clippy", "significant_drop_in_scrutinee"),
    ("clippy", "non_send_fields_in_send_ty"),
    ("clippy", "missing_errors_doc"),
    ("clippy", "missing_panics_doc"),
}

root_manifest = load_toml(RUST_ROOT / "Cargo.toml")
wlints = root_manifest.get("workspace", {})
wlints = wlints.get("lints") if isinstance(wlints, dict) else None
if not isinstance(wlints, dict):
    err("axpoint-core/Cargo.toml 缺少 [workspace.lints]")
else:
    for tool, table in wlints.items():
        if not isinstance(table, dict):
            continue
        for name, val in table.items():
            if isinstance(val, dict):
                extra_keys = set(val) - {"level", "priority"}
                if extra_keys:
                    err(f"[workspace.lints.{tool}] {name}: 只允许 level/priority，发现 {sorted(extra_keys)}")
                level = val.get("level")
            else:
                level = val
            if level not in ALLOWED_LEVELS:
                err(f"[workspace.lints.{tool}] {name} = {level!r}：只允许 deny/forbid，严禁 allow/warn")
    for tool, name in sorted(REQUIRED_LINTS):
        tool_tbl = wlints.get(tool)
        if not isinstance(tool_tbl, dict) or name not in tool_tbl:
            err(f"[workspace.lints.{tool}] 缺少必选门禁项 {name}")
    rust_tbl = wlints.get("rust")
    if not isinstance(rust_tbl, dict) or rust_tbl.get("unsafe_code") != "forbid":
        err("[workspace.lints.rust] unsafe_code 必须为 'forbid'")

DOCTEST_ATTR = "#![doc(test(attr(deny(warnings), forbid(unsafe_code))))]"
for manifest_path in walk_rust_root(lambda f: f == "Cargo.toml"):
    manifest = load_toml(manifest_path)
    raw = manifest_path.read_text(encoding="utf-8")
    if "cargo-features" in manifest:
        err(f"{rel(manifest_path)}: 禁止使用 cargo-features")
    if re.search(r"(?m)^\s*rustflags\s*=", raw):
        err(f"{rel(manifest_path)}: 禁止配置 rustflags")
    if "package" in manifest:
        if manifest.get("lints") != {"workspace": True}:
            err(f"{rel(manifest_path)}: 成员 crate 的 [lints] 必须且只能是 workspace = true")
        lib_rs = manifest_path.parent / "src" / "lib.rs"
        if lib_rs.is_file() and DOCTEST_ATTR not in lib_rs.read_text(encoding="utf-8"):
            err(f"{rel(lib_rs)}: 缺少 {DOCTEST_ATTR}")

# ---------- 3. Cargo 配置与 clippy.toml / rustfmt.toml ----------
BAD_BUILD_KEYS = {"rustflags", "rustdocflags", "rustc", "rustc-wrapper", "rustc-workspace-wrapper", "rustdoc"}
BAD_ALIASES = {"build", "check", "clippy", "doc", "fmt", "test", "b", "c", "d", "t"}
BAD_ENV_RE = re.compile(r"RUSTFLAGS|RUSTDOCFLAGS|RUSTC|RUSTDOC|CLIPPY|RUSTUP_TOOLCHAIN")

cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
cfg_candidates = [
    base / ".cargo" / name
    for base in [RUST_ROOT, REPO_ROOT, *REPO_ROOT.parents]
    for name in ("config", "config.toml")
] + [cargo_home / name for name in ("config", "config.toml")]

seen_cfgs: set[Path] = set()
for cfg_path in cfg_candidates:
    resolved = cfg_path.resolve() if cfg_path.exists() else cfg_path
    if resolved in seen_cfgs or not cfg_path.is_file():
        continue
    seen_cfgs.add(resolved)
    cfg = load_toml(cfg_path)
    build_tbl = cfg.get("build")
    if isinstance(build_tbl, dict):
        for k in sorted(BAD_BUILD_KEYS & set(build_tbl)):
            err(f"{cfg_path}: 禁止配置 build.{k}")
    target_tbl = cfg.get("target")
    if isinstance(target_tbl, dict):
        for tname, tval in target_tbl.items():
            if isinstance(tval, dict):
                for k in sorted({"rustflags", "rustdocflags", "runner"} & set(tval)):
                    err(f"{cfg_path}: 禁止配置 target.{tname}.{k}")
    profile_tbl = cfg.get("profile")
    if isinstance(profile_tbl, dict):
        for pname, pval in profile_tbl.items():
            if isinstance(pval, dict) and "rustflags" in pval:
                err(f"{cfg_path}: 禁止配置 profile.{pname}.rustflags")
    alias_tbl = cfg.get("alias")
    if isinstance(alias_tbl, dict):
        for k in sorted(BAD_ALIASES & set(alias_tbl)):
            err(f"{cfg_path}: 禁止覆盖 alias.{k}")
    env_tbl = cfg.get("env")
    if isinstance(env_tbl, dict):
        for k in sorted(env_tbl):
            if BAD_ENV_RE.search(k):
                err(f"{cfg_path}: 禁止注入 env.{k}")
    if "unstable" in cfg:
        err(f"{cfg_path}: 禁止配置 [unstable]")

STRICT_CLIPPY_MAX = {
    "too-many-lines-threshold": 100,
    "cognitive-complexity-threshold": 25,
    "too-many-arguments-threshold": 7,
    "type-complexity-threshold": 250,
    "max-fn-params-bools": 3,
    "max-struct-bools": 3,
    "enum-variant-size-threshold": 200,
    "max-include-file-size": 65536,
}
FIXED_CLIPPY: dict[str, bool] = {}
EXTRA_CLIPPY = {"disallowed-methods", "disallowed-types", "disallowed-macros", "disallowed-names"}

clippy_confs = [
    p
    for base in (REPO_ROOT, RUST_ROOT)
    for p in (
        [base / "clippy.toml", base / ".clippy.toml"]
        if base == REPO_ROOT
        else list(walk_rust_root(lambda f: f in ("clippy.toml", ".clippy.toml")))
    )
    if p.is_file()
]
clippy_confs = sorted(set(clippy_confs))
expected_clippy = RUST_ROOT / "clippy.toml"
if clippy_confs != [expected_clippy]:
    err(f"clippy 配置必须且只能位于 axpoint-core/clippy.toml，实际 {[rel(c) for c in clippy_confs]}")
if expected_clippy.is_file():
    cc = load_toml(expected_clippy)
    for k, v in cc.items():
        if k in STRICT_CLIPPY_MAX:
            if not isinstance(v, int) or isinstance(v, bool) or not (1 <= v <= STRICT_CLIPPY_MAX[k]):
                err(f"clippy.toml {k} = {v!r}：只能在 1..={STRICT_CLIPPY_MAX[k]} 范围内收紧")
        elif k in FIXED_CLIPPY:
            if v is not FIXED_CLIPPY[k]:
                err(f"clippy.toml {k} 必须为 {FIXED_CLIPPY[k]}")
        elif k not in EXTRA_CLIPPY:
            err(f"clippy.toml 含不允许的放宽键 {k!r}")
    for missing_key in sorted((set(STRICT_CLIPPY_MAX) | set(FIXED_CLIPPY)) - set(cc)):
        err(f"clippy.toml 缺少必选阈值键 {missing_key}")

for fmt_conf in walk_rust_root(lambda f: f in ("rustfmt.toml", ".rustfmt.toml")):
    for bad_key in sorted({"ignore", "disable_all_formatting", "unstable_features"} & set(load_toml(fmt_conf))):
        err(f"{rel(fmt_conf)}: 禁止配置 {bad_key}")

# ---------- 4. Rust 源码扫描（剥离注释与字符串字面量） ----------
TOKEN_RE = re.compile(
    r'//[^\n]*|/\*(?:[^*]|\*(?!/))*\*/|b?r(#*)"(?:.|\n)*?"\1|b?"(?:\\.|[^"\\])*"|'
    r"b?'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]{1,6}\}|.)|[^'\\\n])'",
    re.S,
)


def strip_comments_and_strings(src: str) -> str:
    return TOKEN_RE.sub(
        lambda m: "\n" * m.group(0).count("\n") if m.group(0).startswith("/") else '""',
        src,
    )


SOURCE_RULES = [
    (r"#!?\[\s*(allow|expect|warn)\s*\(", "禁止 allow/expect/warn 属性"),
    (r"\bcfg_attr\s*\((?:[^()]|\([^()]*\))*\b(allow|expect|warn)\s*\(", "禁止 cfg_attr 包装豁免"),
    (r"\bunsafe\b", "禁止 unsafe 关键字"),
    (r"\bcfg(?:_attr)?\s*\([^\]]*?\b(clippy|rustfmt|miri|doc)\b(?!\s*[=(])", "禁止按 clippy/rustfmt/miri/doc 条件编译隐藏代码"),
    (r"\bcfg\s*\(\s*(any\s*\(\s*\)|false)\s*\)", "禁止用 cfg(any())/cfg(false) 隐藏代码"),
    (r"#!?\[\s*(rustfmt|clippy)\s*::", "禁止 rustfmt::/clippy:: 工具属性"),
    (r"#!?\[\s*doc\s*\(\s*hidden", "禁止 #[doc(hidden)] 躲避 missing_docs"),
    (r"#!?\[\s*automatically_derived", "禁止手写 #[automatically_derived]"),
    (r"\binclude\s*!", "禁止使用 include! 绕过静态检查"),
]

for rs_path in walk_rust_root(lambda f: f.endswith(".rs")):
    stripped = strip_comments_and_strings(rs_path.read_text(encoding="utf-8"))
    for pattern, message in SOURCE_RULES:
        for match in re.finditer(pattern, stripped):
            lineno = stripped.count("\n", 0, match.start()) + 1
            err(f"{rel(rs_path)}:{lineno}: {message}")

if errs:
    for item in errs:
        print(f"lint-guard: {item}", file=sys.stderr)
    sys.exit(1)

print("lint-guard: OK")
