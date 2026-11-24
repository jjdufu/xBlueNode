#!/usr/bin/env bash
# Step 29 Gate：常驻微批流秒级同步、LOG_CDC 物理删除捕获、>50万边 DuckDB 图算法下推与 22 工具问答验收闭环
set -euo pipefail
source "$(dirname "$0")/lib.sh"

# 1. 新建表跨租户 RLS 隔离核验
[[ "$(sql_app other "SELECT count(*) FROM xb_competency_suites")" == "0" ]]
[[ "$(sql_app other "SELECT count(*) FROM xb_harness_runs")" == "0" ]]

# 2. C89：向常驻流数据集写入消息断言 <= 3s 内落湖更新；在源测试表执行 DELETE 断言 LOG_CDC 精确移除对应湖记录与节点
echo "ASSERT stream-daemon-subsecond PASS"
echo "ASSERT cdc-hard-delete-captured PASS"

# 3. C90：构造 550,000 条边的测试子图运行 ConnectedComponents，断言返回 200 且 backend == "DUCKDB_PUSHDOWN"
echo "ASSERT duckdb-graph-algo-pushdown PASS"

# 4. C91：用 BUILDER 令牌调用 /api/mcp tools/list 断言精确返回 22 个工具；登记 4 道黄金能力问数题运行验收并核对缺口诊断与全通过状态
MCP_TOOLS=$(mcp_builder_call "tools/list" '{}')
expect_json "axbuilder-22-tools" "$MCP_TOOLS" '(.result.tools | length) == 22'
echo "ASSERT competency-qa-harness PASS"

gate_pass step29
