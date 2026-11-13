#!/usr/bin/env bash
# Sequential full-regression gate runner across all delivered steps.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

export AXPOINT_LLM_CASSETTE="${AXPOINT_LLM_CASSETTE:-$ROOT/tests/cassettes}"

for script in scripts/gate/step[0-9][0-9].sh; do
  [[ -e "$script" ]] || continue
  echo "=== Running $script ==="
  bash "$script"
done

echo "GATE all PASS"
