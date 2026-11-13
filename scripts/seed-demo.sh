#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
MAX_STEP="${1:-99}"
mkdir -p .gate/seeded
touch .gate/ids.env
for f in scripts/seed/[0-9][0-9]-*.sh; do
  [[ -e "$f" ]] || continue
  base="$(basename "$f")"
  step_num="${base%%-*}"
  (( 10#$step_num <= 10#$MAX_STEP )) || continue
  [[ -f ".gate/seeded/$base" ]] && continue
  bash "$f" >> .gate/ids.env
  touch ".gate/seeded/$base"
done
