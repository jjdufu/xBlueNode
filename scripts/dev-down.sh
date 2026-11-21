#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
for _pg_bin in /usr/lib/postgresql/*/bin; do
  [[ -d "$_pg_bin" ]] && export PATH="$_pg_bin:$PATH"
done

if [[ -d .gate/pids ]]; then
  for pidfile in .gate/pids/*; do
    [[ -f "$pidfile" ]] || continue
    kill "$(cat "$pidfile")" 2>/dev/null || true
    rm -f "$pidfile"
  done
fi

if [[ -d .gate/pgdata ]]; then
  pg_ctl -D .gate/pgdata stop -m fast >/dev/null 2>&1 || true
fi

if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  docker compose -f deploy/docker-compose.yml down -v >/dev/null 2>&1 || true
fi

rm -rf .gate .env.test
echo "dev-down: OK"
