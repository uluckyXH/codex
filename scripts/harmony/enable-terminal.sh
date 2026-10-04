#!/bin/sh
# 显式启用或撤销本安装的 zsh 环境；不运行 Codex，不读取账号配置。
set -eu
umask 077
fail() { printf '%s\n' "终端配置失败：$*" >&2; exit 1; }
rc_file="${HOME:?缺少 HOME}/.zshrc"
remove=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --rc-file) [ "$#" -ge 2 ] || fail '缺少配置路径'; rc_file=$2; shift 2 ;;
        --remove) remove=1; shift ;;
        --help) printf '%s\n' '用法：sh enable-terminal.sh [--rc-file 绝对路径] [--remove]'; exit 0 ;;
        *) fail "未知参数：$1" ;;
    esac
done
case "$rc_file" in /*) ;; *) fail '配置路径必须是绝对路径' ;; esac
case "$rc_file" in *'
'*) fail '配置路径不能含换行' ;; esac
[ ! -L "$rc_file" ] || fail '配置文件是符号链接，请显式指定实际文件路径'
if [ -e "$rc_file" ]; then
    [ -f "$rc_file" ] && [ -r "$rc_file" ] && [ -w "$rc_file" ] || fail '配置必须是可读写普通文件'
fi
package=$(CDPATH='' cd -P -- "$(dirname -- "$0")" && pwd)
[ "$remove" -eq 1 ] || [ -f "$package/env.sh" ] || fail '请先运行install.sh，再使用安装目录中的本脚本'
temporary="${rc_file}.codex-$$.tmp"
(set -C; : > "$temporary") || fail '临时文件已存在或配置目录不可写'
trap 'rm -f -- "$temporary"' 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
begin='# >>> 鸿蒙 Codex 环境 >>>'
end='# <<< 鸿蒙 Codex 环境 <<<'
if [ -e "$rc_file" ]; then
    # 只替换完整的专用标记块，标记异常时停止，保留原文件。
    awk -v begin="$begin" -v end="$end" '
        $0 == begin { if (inside || count++) exit 2; inside=1; next }
        $0 == end { if (!inside) exit 2; inside=0; next }
        !inside { print }
        END { if (inside) exit 2 }
    ' "$rc_file" > "$temporary" || fail '发现重复或不完整的 Codex 标记块，未修改配置'
fi
quote() { printf "'"; printf '%s' "$1" | sed "s/'/'\\\\''/g"; printf "'"; }
if [ "$remove" -eq 0 ]; then
    {
        printf '%s\n' "$begin"
        printf '. '; quote "$package/env.sh"; printf '\n'
        printf '%s\n' "$end"
    } >> "$temporary"
fi
command -v zsh >/dev/null 2>&1 || fail '需要 zsh 检查配置语法，未修改配置'
zsh -f -n "$temporary" || fail '合并后的 zsh 语法不合法，未修改配置'
if [ -f "$rc_file" ] && cmp -s "$rc_file" "$temporary"; then
    printf '%s\n' '终端配置已经一致，无需修改。'
    exit 0
fi
if [ -e "$rc_file" ]; then
    backup="${rc_file}.codex-$(date +%Y%m%d-%H%M%S)-$$.bak"
    (set -C; cat "$rc_file" > "$backup") || fail '无法创建原配置备份'
    [ ! -L "$rc_file" ] && cmp -s "$rc_file" "$backup" || fail '配置被其他进程修改，已停止'
    printf '原配置备份：%s\n' "$backup"
fi
mv -- "$temporary" "$rc_file"
if [ "$remove" -eq 1 ]; then
    printf '%s\n' '已撤销本脚本管理的 Codex 启动块；新开终端生效。'
else
    printf '%s\n' '已更新 Codex 启动块；新开终端后直接运行 codex。当前终端可执行：'
    printf '. '; quote "$package/env.sh"; printf '\n'
fi
