#!/usr/bin/env python3
"""xBlueNode 文档链接完整性、项目记忆体积预算与交付文风守卫（Python >= 3.11）。

扫描范围：
- 根文档：`README.md`、`IRON_RULES.md`
- 项目记忆：`AGENTS.md`、`GEMINI.md`、`CLAUDE.md`、`axpoint-core/AGENTS.md`、`axpoint-parser/AGENTS.md`、`xbluenode-web/AGENTS.md`
- 技能与规范：`.agents/**/*.md`、`docs/**/*.md`、`plans/**/*.md`

拦截规则：
1. 所有记忆文件（`AGENTS.md` / `GEMINI.md` / `CLAUDE.md`）必须存在且单文件严格小于 `24,000` 字节（内联规则预算上限）；
2. `docs/blueprint/` 旧混合目录不得重新出现（已完成方案与推进方案必须位于 `plans/`，常驻标准必须位于 `docs/`）；
3. 所有 Markdown 文件（排除代码块内部示例）中的相对文件链接必须指向真实存在的路径；
4. 所有 `docs/` 与 `plans/` 文档禁止出现受众标签、历史迭代残留措辞、夸大宣传形容词与设问句式。
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MAX_RULE_FILE_BYTES = 24_000

REQUIRED_MEMORY_FILES = [
    "AGENTS.md",
    "GEMINI.md",
    "CLAUDE.md",
    "axpoint-core/AGENTS.md",
    "axpoint-parser/AGENTS.md",
    "xbluenode-web/AGENTS.md",
]

REQUIRED_SKILL_FILES = [
    ".agents/skills/execute-plan-step/SKILL.md",
    ".agents/skills/multi-agent-coordinator/SKILL.md",
    ".agents/skills/grounded-ai-checklist/SKILL.md",
]

MD_LINK_RE = re.compile(r"!?\[[^\]]*\]\(([^)#?\s]+)(?:#[^)]*)?\)")
FENCE_RE = re.compile(r"^\s*(`{3,}|~{3,})")

DOC_HYGIENE_PATTERNS: list[tuple[str, re.Pattern[str]]] = [
    (
        "doc-audience-label",
        re.compile(r"(适用(于)?|面向|专为).{0,8}(非技术|运营人员|新手|小白|内部人员)(专用|使用|编写|设计)?|运营人员专用"),
    ),
    (
        "doc-diff-leakage",
        re.compile(r"(现已|已经?)(统一)?(调整为|放宽到?|修改为|替换为)|旧配置里的|原方案为"),
    ),
    (
        "doc-promo-fluff",
        re.compile(r"旗舰级大模型|日常首选（最快）|综合主力|无需额外部署.{1,10}即可直连"),
    ),
    (
        "doc-rhetorical-qa",
        re.compile(r"(怎么办|如何处理|怎么选)\s*[？?]"),
    ),
]


def rel(path: Path) -> str:
    return str(path.relative_to(REPO_ROOT)) if path.is_relative_to(REPO_ROOT) else str(path)


def iter_non_fenced_lines(text: str) -> list[tuple[int, str]]:
    out: list[tuple[int, str]] = []
    in_fence = False
    fence_marker = ""
    for lineno, line in enumerate(text.splitlines(), 1):
        m = FENCE_RE.match(line)
        if m:
            marker = m.group(1)
            if not in_fence:
                in_fence = True
                fence_marker = marker[0] * len(marker)
            elif line.lstrip().startswith(fence_marker):
                in_fence = False
                fence_marker = ""
            continue
        if not in_fence:
            out.append((lineno, line))
    return out


def check_markdown_file(path: Path, check_hygiene: bool, errors: list[str]) -> None:
    text = path.read_text(encoding="utf-8")
    for lineno, line in iter_non_fenced_lines(text):
        # Strip inline backtick code spans before scanning links
        line_no_code = re.sub(r"`[^`]*`", "", line)
        for m in MD_LINK_RE.finditer(line_no_code):
            target = m.group(1).strip()
            if not target or target.startswith(("http://", "https://", "mailto:", "file://", "conversation://")):
                continue
            resolved = (path.parent / target).resolve()
            if not resolved.exists():
                errors.append(f"{rel(path)}:{lineno}: 相对链接目标不存在 `{target}`")

        if check_hygiene:
            for rule_id, pat in DOC_HYGIENE_PATTERNS:
                hm = pat.search(line)
                if hm:
                    errors.append(
                        f"{rel(path)}:{lineno}: 文档规范违例 [{rule_id}] 命中 {hm.group(0)!r}"
                    )


def main() -> int:
    errors: list[str] = []

    legacy_blueprint_dir = REPO_ROOT / "docs" / "blueprint"
    if legacy_blueprint_dir.exists():
        errors.append(
            "检测到遗留 `docs/blueprint` 目录；已完成与推进中的分步方案必须统一存放在 `plans/` 下"
        )

    for mem_rel in REQUIRED_MEMORY_FILES:
        mem_path = REPO_ROOT / mem_rel
        if not mem_path.is_file():
            errors.append(f"缺少必需的项目级记忆文件: `{mem_rel}`")
            continue
        size = mem_path.stat().st_size
        if size == 0:
            errors.append(f"项目级记忆文件不能为空: `{mem_rel}`")
        elif size >= MAX_RULE_FILE_BYTES:
            errors.append(
                f"`{mem_rel}` 体积为 {size} 字节，超出单文件内联上限 {MAX_RULE_FILE_BYTES} 字节"
            )

    for skill_rel in REQUIRED_SKILL_FILES:
        skill_path = REPO_ROOT / skill_rel
        if not skill_path.is_file():
            errors.append(f"缺少必需的项目级工作流技能文件: `{skill_rel}`")

    md_targets: list[tuple[Path, bool]] = []
    for root_doc in ("README.md", "IRON_RULES.md", *REQUIRED_MEMORY_FILES):
        p = REPO_ROOT / root_doc
        if p.is_file():
            is_mem = p.name in {"AGENTS.md", "GEMINI.md", "CLAUDE.md"}
            md_targets.append((p, not is_mem))

    for folder_name, enforce_hygiene in ((".agents", False), ("docs", True), ("plans", True)):
        folder = REPO_ROOT / folder_name
        if not folder.is_dir():
            errors.append(f"缺少顶层目录: `{folder_name}/`")
            continue
        for md_file in sorted(folder.rglob("*.md")):
            if md_file.is_file():
                skip_hygiene = md_file.name in {"SKILL.md", "AGENTS.md"}
                md_targets.append((md_file, enforce_hygiene and not skip_hygiene))

    seen: set[Path] = set()
    for md_path, check_hygiene in md_targets:
        resolved = md_path.resolve()
        if resolved in seen:
            continue
        seen.add(resolved)
        check_markdown_file(md_path, check_hygiene, errors)

    if errors:
        print("docs-and-plans-guard FAILED:", file=sys.stderr)
        for item in errors:
            print(f"  - {item}", file=sys.stderr)
        return 1

    print(f"docs-and-plans-guard: OK ({len(seen)} markdown files verified)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
