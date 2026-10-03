#!/bin/bash
# 在 Finder 中双击，通过同目录入口启动模拟器内的 Codex。
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "$0")" && pwd)"
exec /bin/bash "$script_dir/run-harmony-codex.sh" "$@"
