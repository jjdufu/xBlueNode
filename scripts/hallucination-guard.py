#!/usr/bin/env python3
"""xBlueNode 本体级无幻觉 AI 静态防线守卫（Python >= 3.11，仅依赖标准库）。

审计 `axpoint-core` 中与大模型调用、智能问数、语义编译及智能体运行时相关的关键防线，
确保任何重构不会意外拆除以下五道防幻觉与安全锚点：
1. NL2SQL 必须接入 `sql_guard::guard` 语法树白名单与自修复循环；
2. 人工纠错 SQL 入库前必须经 `validate_corrected_sql` 守卫校验；
3. 数据结果自然语言总结必须经 `verify_or_fallback_summary` 单元格数值接地核验；
4. 系统提示词必须包含不确定澄清（`sql` 置空）与严禁编造未出现数字的硬性指令；
5. `LlmRouter` 出域请求必须接入 `redact_messages_for_egress` 与 `restore_from_egress` 双向保护；
6. `AgentRuntime` 结构化输出与发布端点调用必须接入 `validate_instance_against_schema` JSON Schema 校验。
"""
from __future__ import annotations

import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

REQUIRED_ANCHORS: list[tuple[str, list[str], str]] = [
    (
        "axpoint-core/crates/axpoint-analytics/src/nl2sql/mod.rs",
        [
            "verify_or_fallback_summary",
            "guard_and_explain_session_loop",
            "validate_corrected_sql",
            "build_clarification_output",
        ],
        "NL2SQL 流水线必须保留 AST 守卫、EXPLAIN 预检、不确定澄清分支、纠错校验与后置数值核验接入",
    ),
    (
        "axpoint-core/crates/axpoint-analytics/src/nl2sql/chart.rs",
        [
            "pub fn verify_or_fallback_summary",
            "collect_grounded_cell_values",
            "is_token_grounded",
            "fallback_table_summary",
        ],
        "结果摘要核验器必须保留单元格数值池提取与未接地数字回退逻辑",
    ),
    (
        "axpoint-core/crates/axpoint-analytics/src/nl2sql/prompt.rs",
        [
            "严禁编造或推算未出现的数字",
            "令 sql 为空字符串",
        ],
        "NL2SQL 与摘要提示词必须保留严禁编造数字与不确定时令 sql 为空的核心约束",
    ),
    (
        "axpoint-core/crates/axpoint-analytics/src/semantic.rs",
        [
            "pub fn validate_metric_def",
            "inspect_sql_expression",
            "CompiledMetricQuery",
        ],
        "语义指标层必须保留聚合函数/列存在性静态校验与确定性 SQL 编译器",
    ),
    (
        "axpoint-core/crates/axpoint-llm/src/router.rs",
        [
            "redact_messages_for_egress",
            "restore_turn_from_egress",
            "wrap_egress_stream_tx",
        ],
        "LlmRouter 必须保留出域消息 PII 符号化替换与同步/流式还原防线",
    ),
    (
        "axpoint-core/crates/axpoint-agent/src/runtime.rs",
        [
            "validate_instance_against_schema",
        ],
        "AgentRuntime 必须保留 JSON Schema 结构化输出校验防线",
    ),
    (
        "axpoint-core/crates/axpoint-agent/src/mcp_server.rs",
        [
            "validate_instance_against_schema",
            "PiiRawRead",
        ],
        "McpServerHub 必须保留工具入参 Schema 校验与未授权 PII 属性掩码保护",
    ),
]


def main() -> int:
    errors: list[str] = []

    for rel_path, tokens, desc in REQUIRED_ANCHORS:
        full_path = REPO_ROOT / rel_path
        if not full_path.is_file():
            errors.append(f"缺少关键防幻觉源码文件 `{rel_path}` ({desc})")
            continue
        content = full_path.read_text(encoding="utf-8")
        for token in tokens:
            if token not in content:
                errors.append(
                    f"`{rel_path}` 缺失防幻觉关键锚点 `{token}` —— {desc}"
                )

    if errors:
        print("hallucination-guard FAILED:", file=sys.stderr)
        for item in errors:
            print(f"  - {item}", file=sys.stderr)
        return 1

    print("hallucination-guard: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
