#!/bin/sh
# 只安装当前目录包，不运行程序、不读取 Codex 配置或认证文件。
set -eu
umask 077

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
[ -f "$package/文件校验清单.sha256" ] && [ ! -L "$package/文件校验清单.sha256" ] || fail '缺少文件校验清单'
[ -f "$package/bin/codex" ] && [ -f "$package/codex-resources/bwrap" ] && [ -f "$package/codex-path/rg" ] && [ -f "$package/codex-resources/harmony-runtime-probe" ] || fail '程序包不完整（含独立目录探针）'


# 只消费校验清单的普通文件，额外文件不会进入安装目录。
# 先验证全部条目，再校验内容；禁止通过清单路径走出程序包。
each_file() {
    while IFS= read -r entry || [ -n "$entry" ]; do
        case "$entry" in *"  "*) ;; *) fail '校验清单格式错误' ;; esac
        checksum=${entry%%  *}
        name=${entry#*  }
        [ "${#checksum}" -eq 64 ] || fail 'SHA-256 长度错误'
        case "$checksum" in *[!0123456789abcdefABCDEF]*) fail 'SHA-256 格式错误' ;; esac
        case "$name" in ''|/*|环境.sh|文件校验清单.sha256) fail "非法或保留路径：$name" ;; esac
        case "/$name/" in *'/../'*|*'/./'*|*'//'*) fail "非法相对路径：$name" ;; esac
        "$1" "$name"
    done < "$package/文件校验清单.sha256"
}
validate_file() {
    entry_path="$package/$1"
    [ -f "$entry_path" ] || fail "清单条目不是普通文件：$1"
    while [ "$entry_path" != "$package" ]; do
        [ ! -L "$entry_path" ] || fail "清单条目不能经过符号链接：$1"
        entry_path=${entry_path%/*}
    done
}
copy_file() {
    mkdir -p -- "$(dirname -- "$prefix/$1")"
    cp -p -- "$package/$1" "$prefix/$1" || fail '复制失败；保留已复制目录供排查'
}
each_file validate_file

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
each_file copy_file
cp -p -- "$package/文件校验清单.sha256" "$prefix/文件校验清单.sha256"
verify "$prefix" || fail '安装后校验失败；保留目录供排查'
[ -x "$prefix/bin/codex" ] && [ -x "$prefix/codex-resources/bwrap" ] && [ -x "$prefix/codex-path/rg" ] && [ -x "$prefix/codex-resources/harmony-runtime-probe" ] || fail '复制后可执行权限丢失'

# 单引号中的路径字面量不展开 $、反引号或命令替换。
quote() { printf "'"; printf '%s' "$1" | sed "s/'/'\\\\''/g"; printf "'"; }
(
set -C
{
    printf 'case ":${PATH-}:" in\n    *:'
    quote "$prefix/bin"
    printf ':*) ;;\n    *) export PATH='
    quote "$prefix/bin"
    printf ':"${PATH-}" ;;\nesac\n'
} > "$prefix/环境.sh"
) || fail '环境脚本已存在或无法创建，未覆盖原文件'
printf '\n%s\n' '文件安装与摘要检查完成；尚未验证鸿蒙设备的签名接受和运行能力。'
printf '%s\n' '在当前终端执行以下命令，然后运行 codex --version：'
printf '. '; quote "$prefix/环境.sh"; printf '\n'
printf '%s\n' '需要新终端也能找到 codex 时，将上面这一行添加到自己的 ~/.zshrc；不要覆盖已有内容。'
if [ -f "$prefix/启用终端.sh" ]; then
    printf '%s\n' '也可执行以下命令，仅更新带标记的 Codex 启动块并备份原文件：'
    printf 'sh '; quote "$prefix/启用终端.sh"; printf '\n'
fi
