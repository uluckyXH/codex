#!/bin/sh
# 本机诊断：不调用模型、不读取认证文件；--sandbox 额外执行一次受限 pwd。
set -eu
umask 077
fail() { printf '%s\n' "诊断失败：$*" >&2; exit 1; }
output=''
sandbox=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output-dir) [ "$#" -ge 2 ] || fail '缺少输出路径'; output=$2; shift 2 ;;
        --sandbox) sandbox=1; shift ;;
        --help) printf '%s\n' '用法：sh 诊断.sh [--sandbox] [--output-dir 全新绝对目录]'; exit 0 ;;
        *) fail "未知参数：$1" ;;
    esac
done
package=$(CDPATH='' cd -P -- "$(dirname -- "$0")" && pwd)
[ -x "$package/bin/codex" ] || fail '缺少可执行主程序，请使用完整安装目录'
if [ -z "$output" ]; then
    output="${HOME:?缺少 HOME}/Codex诊断/$(date +%Y%m%d-%H%M%S)-$$"
fi
case "$output" in /*) ;; *) fail '输出目录必须为绝对路径' ;; esac
[ ! -e "$output" ] && [ ! -L "$output" ] || fail '输出目录已存在，请换一个新目录'
mkdir -p -- "$(dirname -- "$output")"
mkdir -- "$output"
output=$(CDPATH='' cd -P -- "$output" && pwd)
failed=0
run_check() {
    name=$1; shift
    if "$@" > "$output/$name.txt" 2>&1; then code=0; else code=$?; fi
    printf '%s：退出码 %s\n' "$name" "$code" >> "$output/检查摘要.txt"
    [ "$code" -eq 0 ] || failed=$((failed + 1))
}
{
    printf '生成时间（UTC）：'; date -u '+%Y-%m-%dT%H:%M:%SZ'
    printf '系统与内核：'; uname -sr
    printf '架构：'; uname -m
    printf '安装目录：%s\n' "$package"
    printf 'TMPDIR：%s\n' "${TMPDIR-未设置，由程序选择}"
    printf 'CODEX_HOME：%s\n' "${CODEX_HOME-未设置}"
    if [ -n "${CODEX_CA_CERTIFICATE-}" ]; then
        ca_path=$CODEX_CA_CERTIFICATE; ca_source=CODEX_CA_CERTIFICATE
    elif [ -n "${SSL_CERT_FILE-}" ]; then
        ca_path=$SSL_CERT_FILE; ca_source=SSL_CERT_FILE
    else
        ca_path=/etc/ssl/certs/cacert.pem; ca_source=鸿蒙系统默认
    fi
    printf 'CA 来源：%s\nCA 路径：%s\n' "$ca_source" "$ca_path"
    if [ -f "$ca_path" ] && [ -r "$ca_path" ]; then printf 'CA 文件可读\n'; else printf 'CA 文件不存在或不可读\n'; fi
    printf '未采集 API Key、认证文件、配置正文、提示词或全部环境变量。\n'
} > "$output/运行环境.txt"
run_check 版本 "$package/bin/codex" --version
run_check 构建与能力 "$package/bin/codex" doctor --capabilities --json
run_check 包内沙箱版本 "$package/codex-resources/bwrap" --version
if [ -f "$package/文件校验清单.sha256" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
        run_check 安装文件校验 sh -c 'cd "$1" && sha256sum -c 文件校验清单.sha256' sh "$package"
    elif command -v shasum >/dev/null 2>&1; then
        run_check 安装文件校验 sh -c 'cd "$1" && shasum -a 256 -c 文件校验清单.sha256' sh "$package"
    fi
fi
if [ "$sandbox" -eq 1 ]; then
    mkdir -- "$output/沙箱工作目录"
    probe_shell=/usr/bin/sh
    [ -x "$probe_shell" ] || probe_shell=/bin/sh
    printf '受限执行 Shell：%s\n' "$probe_shell" >> "$output/运行环境.txt"
    # 固定只执行 pwd；保留受限策略，不以危险全盘权限回退。
    run_check 普通终端 sh -c 'cd "$1" && "$2" -c pwd' sh "$output/沙箱工作目录" "$probe_shell"
    run_check 受限终端 sh -c 'cd "$1" && CODEX_HARMONY_PROCESS_DIAGNOSTICS=1 "$2" -c sandbox_mode="\"read-only\"" sandbox -- "$3" -c pwd' sh "$output/沙箱工作目录" "$package/bin/codex" "$probe_shell"
fi
printf '\n%s\n' '这些检查不代表真实模型调用或交互工具闭环已通过。' >> "$output/检查摘要.txt"
cat "$output/检查摘要.txt"
printf '\n诊断文件：%s\n' "$output"
printf '%s\n' '分享前检查本机路径和错误内容；无需附带 config.toml 或 auth.json。'
[ "$failed" -eq 0 ]
