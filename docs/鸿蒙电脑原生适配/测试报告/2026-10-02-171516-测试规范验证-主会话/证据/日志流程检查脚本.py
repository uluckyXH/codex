"""直接提取测试规范中的函数，验证退出码、参数边界和防止覆盖的行为。"""

import argparse
from pathlib import Path
import re
import shlex
import subprocess
import sys


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--output-dir", type=Path, required=True)
args = parser.parse_args()
root = next(
    p for p in Path(__file__).resolve().parents if (p / "codex-rs/rust-toolchain.toml").is_file()
)
text = (root / "docs/鸿蒙电脑原生适配/测试规范.md").read_text()
match = re.search(r"```sh\n(harmony_run_case\(\) \{.*?\n\})\n```", text, re.S)
assert match is not None, "找不到规范中的日志函数"
literal = "中文 参数 ' 引号 $(printf 不应执行)"
success = shlex.join([sys.executable, "-c", "import sys; print(sys.argv[1])", literal])
failure = shlex.join([sys.executable, "-c", "import sys; sys.exit(7)"])
for shell in ("/bin/sh", "/bin/zsh"):
    output = (args.output_dir / Path(shell).name).resolve()
    (output / "日志").mkdir(parents=True, exist_ok=False)
    script = f"""
harmony_run_root={shlex.quote(str(output))}
{match.group(1)}
harmony_run_case 成功用例 {success}
harmony_ok=$?
harmony_run_case 预期非零用例 {failure}
harmony_failure=$?
harmony_run_case 成功用例 {failure}
harmony_duplicate=$?
test "$harmony_ok" -eq 0 && test "$harmony_failure" -eq 7 && test "$harmony_duplicate" -eq 125
"""
    result = subprocess.run([shell], input=script, text=True, capture_output=True, cwd=root)
    assert result.returncode == 0, (shell, result.stdout, result.stderr)
    rows = (output / "执行结果.tsv").read_text().splitlines()
    assert len(rows) == 2, rows
    assert [row.split("\t")[-1] for row in rows] == ["0", "7"], rows
    success_log = (output / "日志/成功用例.log").read_text()
    failure_log = (output / "日志/预期非零用例.log").read_text()
    assert literal in success_log, success_log
    assert success_log.endswith("退出码：0\n"), success_log
    assert failure_log.endswith("退出码：7\n"), failure_log
    assert "用例日志已存在" in result.stderr, result.stderr
    print(f"{shell}：成功码 0、预期非零码 7、重复日志拒绝码 125 均符合断言；参数原样保留。")
print("日志流程检查通过：两个 Shell，各覆盖三个退出场景。非零码 7 是人为注入的工具测试。")
