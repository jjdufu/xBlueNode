#!/usr/bin/env bash
# 测量 AST + 本体法定 JOIN + 单元格数值核对 + 引用角标核验 1,000 次循环 P50/P95/P99 延迟
set -euo pipefail
cd "$(dirname "$0")/../.."
(cd axpoint-core && \
  cargo test -p axpoint-analytics --release --locked --test grounded_guard_bench -- --nocapture \
  | grep -E '^GROUNDED_BENCH_JSON=' | tail -n1 | sed 's/^GROUNDED_BENCH_JSON=//')
