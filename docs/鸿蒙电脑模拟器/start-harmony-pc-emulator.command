#!/bin/bash
# 在 Finder 中双击此文件，使用同目录的启动脚本。
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "$0")" && pwd)"
exec /bin/bash "$script_dir/start-harmony-pc-emulator.sh" "$@"
