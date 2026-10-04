#!/usr/bin/env python3
"""检查本批中文报告附件、本地引用与已经验证的源码身份。"""

import hashlib
import json
from pathlib import Path
import re
import subprocess

REPORT = Path(__file__).resolve().parent
REPO = REPORT.parents[3]


def main():
    for path in REPORT.iterdir():
        if path.is_file() and path.suffix in {".md", ".json", ".py"}:
            text = path.read_text()
            if not text.endswith("\n") or any(line.rstrip() != line for line in text.splitlines()):
                raise RuntimeError(f"文档空白格式异常：{path.name}")
            if path.suffix == ".json":
                json.loads(text)
    text = (REPORT / "测试报告.md").read_text()
    if "待填写" in text or "本文件是模板" in text:
        raise RuntimeError("报告仍含模板占位内容")
    for target in re.findall(r"\]\(([^)]+)\)", text):
        if not (REPORT / target).is_file():
            raise RuntimeError(f"本地引用不存在：{target}")
    identity = json.loads((REPORT / "源码身份.json").read_text())
    for relative, expected in identity["文件SHA256"].items():
        actual = hashlib.sha256((REPO / relative).read_bytes()).hexdigest()
        if actual != expected:
            raise RuntimeError(f"被测源码与交付内容不一致：{relative}")
    commands = json.loads((REPORT / "命令与结果.json").read_text())
    for command in commands:
        if command["exit_code"] != 0:
            raise RuntimeError(f"本批存在失败记录：{command['case']}")
    subprocess.run(["git", "diff", "--check"], cwd=REPO, check=True)
    print("中文报告、本地链接、JSON 附件与被测源码摘要检查通过。")
    print("未运行 Rust 测试、目标程序或认证流程。")


if __name__ == "__main__":
    main()
