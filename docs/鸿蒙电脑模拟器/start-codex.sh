#!/system/bin/sh
# 此脚本安装到模拟器后运行；Mac 端请使用 run-harmony-codex.sh。
set -eu
umask 077

fail() { printf '%s\n' "Codex 启动失败：$*" >&2; exit 1; }
case "${1-}" in
    --help)
        printf '%s\n' '在鸿蒙模拟器 hdc 交互终端内运行：sh /data/local/tmp/codex-emulator/start-codex.sh'
        printf '%s\n' '固定模型 gpt-5.6-terra；读取专用配置和工作区，不自动改变沙箱模式。'
        exit 0 ;;
    '') ;;
    *) fail '此启动入口不接受 Codex 参数；请在专用配置中调整设置。' ;;
esac
[ "$#" -le 1 ] || fail '参数数量不正确'
[ -t 0 ] && [ -t 1 ] || fail '需要交互终端；请使用 Mac 端 run-harmony-codex.sh 连接。'

root=/data/local/tmp/codex-emulator
[ -f "$root/active-package" ] && [ ! -L "$root/active-package" ] || fail '缺少已验证的安装记录'
IFS= read -r package_name < "$root/active-package" || fail '安装记录不完整'
case "$package_name" in
    ''|*[!A-Za-z0-9._-]*) fail '安装记录含非法名称' ;;
    codex-*) ;;
    *) fail '安装记录不是 Codex 版本目录' ;;
esac
package="$root/packages/$package_name"
[ -x "$package/bin/codex" ] || fail '安装目录缺少可执行 Codex'
[ -f "$root/state/config.toml" ] || fail '尚未安装配置文件'
for directory in home state workspace logs tmp; do
    [ -d "$root/$directory" ] && [ ! -L "$root/$directory" ] || fail "缺少专用目录：$directory"
done

export HOME="$root/home"
export CODEX_HOME="$root/state"
export TMPDIR="$root/tmp"
export SHELL=/system/bin/sh
export PATH="$package/bin:$package/codex-path:${PATH:-/system/bin}"
case "${TERM-}" in ''|dumb) export TERM=xterm-256color ;; esac
cd "$root/workspace"
exec "$package/bin/codex" --model gpt-5.6-terra --no-alt-screen \
    -c 'log_dir="/data/local/tmp/codex-emulator/logs"'
