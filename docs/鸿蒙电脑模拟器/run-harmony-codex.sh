#!/bin/bash
# 从 Mac 的真实终端进入已连接模拟器中的 Codex，不读取认证配置。
set -euo pipefail

die() { printf '错误：%s\n' "$*" >&2; exit 1; }

if [[ $# -eq 1 && ( "$1" == --help || "$1" == -h ) ]]; then
  printf '%s\n' \
    '用法：bash run-harmony-codex.sh' \
    '在已运行的鸿蒙 PC 模拟器中交互启动 Codex；不启动或切换模拟器。' \
    '远端入口：/data/local/tmp/codex-emulator/start-codex.sh' \
    '默认目标：127.0.0.1:5555' \
    '可覆盖：HARMONY_HDC（hdc 完整路径）、HARMONY_HDC_TARGET（连接标识）。' \
    '请在 Mac 的终端窗口中执行；退出 Codex 后返回原终端并保留退出码。' \
    '本脚本不修改模型、API Key 或沙箱权限。'
  exit 0
fi
[[ $# -eq 0 ]] || die '不支持额外参数；使用 --help 查看用法。'
[[ "$(uname -s)" == Darwin ]] || die '此入口用于 Mac 终端。'
[[ -t 0 && -t 1 ]] || die '需要交互终端，请在 Mac 终端中执行或双击 .command 文件，不要重定向输入输出。'

script_dir="$(cd -- "$(dirname -- "$0")" && pwd)"
hdc="${HARMONY_HDC:-/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/toolchains/hdc}"
target="${HARMONY_HDC_TARGET:-127.0.0.1:5555}"
[[ "$hdc" == /* && -f "$hdc" && -x "$hdc" ]] || die "找不到可执行的 hdc，请通过 HARMONY_HDC 指定完整路径：$hdc"
[[ -x /usr/bin/expect ]] || die '找不到 /usr/bin/expect。'
[[ -r "$script_dir/run-harmony-codex.exp" ]] || die '缺少同目录的 run-harmony-codex.exp。'
target_pattern='^([A-Za-z0-9_.:-]|\[|\])+$'
[[ "$target" =~ $target_pattern && "$target" != -* ]] || die 'HARMONY_HDC_TARGET 不是有效的连接标识。'

printf '正在连接鸿蒙 Codex：%s\n' "$target"
# 以独立参数传递路径和标识，避免 Shell/Tcl 重新解释用户提供的值。
exec /usr/bin/expect "$script_dir/run-harmony-codex.exp" \
  "$hdc" "$target" /data/local/tmp/codex-emulator/start-codex.sh
