#!/bin/sh
# 只安装当前目录包，不运行程序、不读取 Codex 配置或认证文件。
set -eu

fail() { printf '%s\n' "安装失败：$*" >&2; exit 1; }
usage() {
    printf '%s\n' '用法：sh 安装.sh --prefix 绝对安装路径'
    printf '%s\n' '安装到全新目录；完成后按输出加载环境.sh。已有目录不会覆盖。'
}
prefix=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        --prefix) [ "$#" -ge 2 ] || fail '--prefix 缺少路径'; prefix=$2; shift 2 ;;
        --help) usage; exit 0 ;;
        *) usage >&2; fail "未知参数：$1" ;;
    esac
done
[ -n "$prefix" ] || { usage >&2; fail '请指定 --prefix'; }
case "$prefix" in /*) ;; *) fail '安装路径必须是绝对路径' ;; esac
case "$prefix" in *:*) fail '安装路径不能含 PATH 分隔符冒号' ;; esac
# 换行不能安全写入一行 Shell 环境配置；空格、中文、单引号均支持。
case "$prefix" in *'
'*) fail '安装路径不能含换行' ;; esac
[ ! -e "$prefix" ] && [ ! -L "$prefix" ] || fail '安装目录已存在，请使用新的版本目录'
package=$(CDPATH='' cd -P -- "$(dirname -- "$0")" && pwd)
[ -f "$package/文件校验清单.sha256" ] || fail '缺少文件校验清单'
[ -f "$package/bin/codex" ] && [ -f "$package/codex-resources/bwrap" ] && [ -f "$package/codex-path/rg" ] || fail '程序包不完整'

verify() {
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$1" && sha256sum -c 文件校验清单.sha256)
    elif command -v shasum >/dev/null 2>&1; then
        (cd "$1" && shasum -a 256 -c 文件校验清单.sha256)
    else
        fail '终端需要 sha256sum 或 shasum 才能检查文件完整性'
    fi
}
verify "$package" || fail '源程序包校验失败'
mkdir -p -- "$(dirname -- "$prefix")"
# mkdir 的失败不清理已有目录，避免并发安装时删到别人的文件。
mkdir -- "$prefix" || fail '无法创建新的安装目录'
cp -R "$package/." "$prefix/" || fail '复制失败；保留已复制目录供排查'
verify "$prefix" || fail '安装后校验失败；保留目录供排查'
[ -x "$prefix/bin/codex" ] && [ -x "$prefix/codex-resources/bwrap" ] && [ -x "$prefix/codex-path/rg" ] || fail '复制后可执行权限丢失'

# 单引号中的路径字面量不展开 $、反引号或命令替换。
quote() { printf "'"; printf '%s' "$1" | sed "s/'/'\\\\''/g"; printf "'"; }
{
    printf 'export PATH='
    quote "$prefix/bin"
    printf ':"$PATH"\n'
} > "$prefix/环境.sh"
printf '\n%s\n' '文件安装与摘要检查完成；尚未验证鸿蒙设备的签名接受和运行能力。'
printf '%s\n' '在当前终端执行以下命令，然后运行 codex --version：'
printf '. '; quote "$prefix/环境.sh"; printf '\n'
printf '%s\n' '需要新终端也能找到 codex 时，将上面这一行添加到自己的 ~/.zshrc；不要覆盖已有内容。'
