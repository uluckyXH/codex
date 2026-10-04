#!/usr/bin/env python3
"""为本批定向检查保存独立日志、实际退出码和北京时间。"""
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo
import os
import shlex
import subprocess
import sys

report = Path(__file__).resolve().parents[1]
root = report.parents[3]
output = root / ".harmony-build/测试批次" / report.name
name, *command = sys.argv[1:]
env = os.environ.copy()
env.update(RUSTUP_HOME="/Volumes/MacSSD/dev/rust/rustup",
           CARGO_HOME="/Volumes/MacSSD/dev/rust/cargo", RUSTUP_AUTO_INSTALL="0",
           CARGO_BUILD_JOBS="2", CARGO_NET_OFFLINE="true",
           CARGO_TARGET_DIR=str(output / "宿主产物"), TMPDIR=str(output / "临时目录"))
env["PATH"] = "/opt/homebrew/opt/rustup/bin:/Volumes/MacSSD/dev/rust/cargo/bin:" + env["PATH"]
now = lambda: datetime.now(ZoneInfo("Asia/Shanghai")).isoformat(timespec="seconds")
started = now()
log = output / "日志" / (name + ".log")
with log.open("x") as stream:
    stream.write(f"工作目录：{root}\n开始时间：{started}\n命令：{shlex.join(command)}\n")
    stream.flush()
    result = subprocess.run(command, cwd=root, env=env, stdout=stream, stderr=subprocess.STDOUT)
    finished = now()
    stream.write(f"\n结束时间：{finished}\n退出码：{result.returncode}\n")
with (output / "执行结果.tsv").open("a") as stream:
    stream.write(f"{name}\t{started}\t{finished}\t{result.returncode}\n")
print(f"{name}：退出码 {result.returncode}；日志 {log}")
sys.exit(result.returncode)
