#!/usr/bin/env bash
# Step 10 §10.11 AxSim pure kernel benchmark: 50,000 nodes / 200,000 links / 100 cycles (size 10..=50).
# Asserts steady P95 <= 2s and 200-sample uncertainty P95 <= 20s, writing results JSON.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/axpoint-core/target}"
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-0}"
export CARGO_PROFILE_TEST_DEBUG="${CARGO_PROFILE_TEST_DEBUG:-0}"

ITERS="${BENCH_ITERS:-20}"
GIT_SHA="$(git -C "$ROOT" rev-parse --short HEAD)"
mkdir -p "$ROOT/scripts/bench/results"
OUT_FILE="$ROOT/scripts/bench/results/sim-${GIT_SHA}.json"

export AXSIM_BENCH_ITERS="$ITERS"
export AXSIM_BENCH_OUT="$OUT_FILE"

(
  cd "$ROOT/axpoint-core"
  cargo test -p axpoint-domain --release --locked --test sim \
    bench_sim_50k_nodes_200k_links_100_cycles -- --ignored --nocapture
)

echo "Benchmark results written to: $OUT_FILE"
