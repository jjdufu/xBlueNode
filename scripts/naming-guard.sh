#!/usr/bin/env bash
# xBlueNode / AxPoint 自主命名守卫。
# 禁用词正则只从环境变量 NAMING_DENYLIST 读取（CI 中来自仓库变量 vars.NAMING_DENYLIST，
# 本地来自未被 Git 跟踪的 .env），仓库内不保存任何禁用词明文。
# 用法：NAMING_DENYLIST='<扩展正则>' bash scripts/naming-guard.sh [扫描目录，默认仓库根目录]
set -euo pipefail

ROOT="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

if [[ -z "${NAMING_DENYLIST:-}" && -f "$ROOT/.env" ]]; then
  NAMING_DENYLIST="$(grep -E '^NAMING_DENYLIST=' "$ROOT/.env" | head -n 1 | cut -d= -f2- | sed -e "s/^['\"]//" -e "s/['\"]$//")"
fi

if [[ -z "${NAMING_DENYLIST:-}" ]]; then
  echo "naming-guard: NAMING_DENYLIST 未配置（CI：仓库 Settings → Variables → NAMING_DENYLIST；本地：导出环境变量或写入 .env）" >&2
  exit 2
fi

EXCLUDES=(--exclude-dir=.git --exclude-dir=target --exclude-dir=node_modules --exclude-dir=dist
          --exclude-dir=__pycache__ --exclude-dir=.venv --exclude=.env --exclude=package-lock.json)

if grep -rnI -iE "$NAMING_DENYLIST" "${EXCLUDES[@]}" "$ROOT"; then
  echo "naming-guard: 检出外部品牌或衍生命名，请改用 xBlueNode / AxPoint 自主命名体系。" >&2
  exit 1
fi

echo "naming-guard: OK"
