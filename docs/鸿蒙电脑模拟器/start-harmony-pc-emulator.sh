#!/bin/bash
# 在 Mac 上启动已有的鸿蒙 7 PC 模拟器，保留实例和用户数据。
set -euo pipefail

die() { printf '错误：%s\n' "$*" >&2; exit 1; }

mode="${1:-start}"
case "$mode" in
  start|--status|--dry-run) ;;
  -h|--help)
    printf '%s\n' \
      '用法：bash start-harmony-pc-emulator.sh [--status|--dry-run]' \
      '不带参数：已经运行就复用，否则启动已有实例。' \
      '--status：只检查运行状态和 hdc 连接。' \
      '--dry-run：只打印启动命令，不调用模拟器或 hdc。' \
      '可覆盖：DEVECO_APP、HARMONY_EMULATOR_NAME、HARMONY_EMULATOR_ROOT、HARMONY_EMULATOR_IMAGE_ROOT、HARMONY_EMULATOR_LOG_DIR。'
    exit 0 ;;
  *) die "不支持的参数：$mode；使用 --help 查看用法。" ;;
esac
[[ $# -le 1 ]] || die '一次只能提供一个操作参数。'
[[ "$(uname -s)" == Darwin ]] || die '此脚本用于 Apple 芯片 Mac。'

# 使用这台 Mac 已有的外置盘安装，可通过环境变量换成自己的路径。
app="${DEVECO_APP:-/Volumes/MacSSD/Applications/DevEco-Studio.app}"
name="${HARMONY_EMULATOR_NAME:-MateBook Pro}"
instance_root="${HARMONY_EMULATOR_ROOT:-/Volumes/MacSSD/Huawei}"
image_root="${HARMONY_EMULATOR_IMAGE_ROOT:-$instance_root}"
log_root="${HARMONY_EMULATOR_LOG_DIR:-/Volumes/MacSSD/鸿蒙模拟器日志}"

# /Applications 中可能只有启动壳；读取实际路径，不执行配置文件。
if [[ -f "$app/Contents/Resources/real_app_path.txt" ]]; then
  app="$(cat "$app/Contents/Resources/real_app_path.txt")"
fi
emulator="$app/Contents/tools/emulator/Emulator"
hdc="$app/Contents/sdk/default/openharmony/toolchains/hdc"
[[ -x "$emulator" ]] || die "找不到模拟器工具：$emulator"
[[ -x "$hdc" ]] || die "找不到调试工具：$hdc"
[[ -f "$instance_root/$name/config.ini" ]] || die "找不到已有实例：$instance_root/$name/config.ini"
[[ -d "$image_root" ]] || die "找不到镜像目录：$image_root"

# 使用实例的现有启动方式，不传 reset、不删除数据、不自动接受许可协议。
start_command=("$emulator" -start "$name" -instancePath "$instance_root" -imageRoot "$image_root")
if [[ "$mode" == --dry-run ]]; then
  printf '将使用的启动命令：\n'
  printf '%q ' "${start_command[@]}"
  printf '\n启动日志目录：%s\n' "$log_root"
  exit 0
fi

command -v python3 >/dev/null 2>&1 || die '需要 python3 读取官方工具返回的 JSON；当前 Mac 的开发工具已提供该命令。'

# 精确匹配实例名称和路径，避免误用同名实例或把其他设备当成 PC。
# 工具可能先输出镜像目录提示，因此只解析随后完整的 JSON 数组。
details="$("$emulator" -list -details 2>&1)" || die '读取模拟器列表失败。请先在 DevEco Studio 的 Device Manager 检查实例。'
running="$(printf '%s' "$details" | python3 -c '
import json
import os
import sys

text = sys.stdin.read()
for failure in ("Cannot get process list", "Operation not permitted", "sysmon request failed"):
    if failure in text:
        sys.exit("无法可靠读取模拟器进程状态，请在 Mac 普通终端中执行；本次不重复启动。")
decoder = json.JSONDecoder()
records = None
offset = 0
for line in text.splitlines(keepends=True):
    if line.lstrip().startswith("["):
        try:
            value, _ = decoder.raw_decode(text[offset:].lstrip())
        except json.JSONDecodeError:
            pass
        else:
            if isinstance(value, list) and all(isinstance(item, dict) for item in value):
                records = value
                break
    offset += len(line)
if records is None:
    sys.exit("官方模拟器工具未返回可识别的实例列表。")
name, root = sys.argv[1:]
expected = os.path.realpath(os.path.join(root, name))
matches = [item for item in records if item.get("name") == name
           and os.path.realpath(item.get("instancePath", "")) == expected]
if len(matches) != 1:
    sys.exit("官方列表中未找到唯一匹配的实例；请在 Device Manager 核对名称及实例路径。")
item = matches[0]
if item.get("deviceType") != "2in1" or not str(item.get("os.osVersion", "")).startswith("HarmonyOS 7."):
    sys.exit("所选实例不是鸿蒙 7 的 2in1 设备。")
if item.get("hw.cpu.arch") != "arm64":
    sys.exit("所选实例不是 ARM64 架构。")
state = str(item.get("isRunning", "")).lower()
if state not in ("true", "false"):
    sys.exit("官方工具未返回明确的运行状态；本次不重复启动。")
print(state)
' "$name" "$instance_root")" || die '实例检查未通过，未启动模拟器。'

if [[ "$running" == true ]]; then
  printf '模拟器已经运行，复用现有实例：%s\n' "$name"
elif [[ "$mode" == --status ]]; then
  printf '模拟器尚未运行：%s\n' "$name"
else
  # 将启动输出放在外置盘，后台运行，关闭当前终端也不会主动停止模拟器。
  mkdir -p "$log_root"
  log_file="$(mktemp "$log_root/启动-$(date '+%Y%m%d-%H%M%S')-XXXXXX")"
  nohup "${start_command[@]}" </dev/null >"$log_file" 2>&1 &
  launcher_pid=$!
  sleep 2
  if ! kill -0 "$launcher_pid" 2>/dev/null; then
    result=0
    wait "$launcher_pid" || result=$?
    [[ $result -eq 0 ]] || die "启动程序退出，状态为 $result；请查看日志：$log_file"
  fi
  printf '已提交启动请求：%s\n启动日志：%s\n' "$name" "$log_file"
  printf '请等待模拟器完成开机；启动请求成功不代表设备已经连接。\n'
fi

# 只展示当前连接列表，不重启 hdc 服务，也不自动选择或操作其他设备。
printf '\nhdc 当前连接列表：\n'
targets="$("$hdc" list targets -v 2>&1)" || die 'hdc 查询失败，请在 Mac 普通终端重试。'
printf '%s\n' "$targets"
if [[ "$targets" == *'Connect server failed'* ]]; then
  die '连接 hdc 服务失败，请在 Mac 普通终端重试；模拟器数据未更改。'
fi
printf '\n如列表暂未出现 Connected，完成开机后用 --status 再检查。\n'
