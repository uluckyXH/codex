#!/usr/bin/env python3
"""本批平台验证的环境与日志入口；只执行显式传入的命令。"""

import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
from zoneinfo import ZoneInfo


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ohos", action="store_true")
    parser.add_argument("case")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("需要显式命令")
    repo = Path(__file__).resolve().parents[4]
    run = repo / ".harmony-build/测试批次" / Path(__file__).parent.name
    env = dict(os.environ)
    env.pop("CODEX_OHOS_RUNTIME_BASE", None)
    env.update(
        RUSTUP_HOME="/Volumes/MacSSD/dev/rust/rustup",
        CARGO_HOME="/Volumes/MacSSD/dev/rust/cargo",
        RUSTUP_AUTO_INSTALL="0",
        CARGO_BUILD_JOBS="2",
        CARGO_TARGET_DIR=str(run / "宿主产物"),
        TMPDIR='/Volumes/MacSSD/Repositories/codex/.harmony-build/mnt-j2xctjsr',
        PYTHONDONTWRITEBYTECODE="1",
    )
    env["PATH"] = "/opt/homebrew/opt/rustup/bin:/Volumes/MacSSD/dev/rust/cargo/bin:" + env["PATH"]
    if args.ohos:
        sys.path.insert(0, str(repo / "scripts"))
        from build_harmony import build_environment, native_sdk

        sdk = native_sdk(Path("/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native"))
        env = build_environment(sdk, run / "鸿蒙构建", env)
    log = run / "日志" / (args.case + ".log")
    started = datetime.now(ZoneInfo("Asia/Shanghai")).isoformat()
    with log.open("x") as output:
        output.write(f"工作目录：{repo}\n开始时间：{started}\n命令：{shlex.join(command)}\n")
        output.flush()
        result = subprocess.run(command, cwd=repo, env=env, stdout=output, stderr=subprocess.STDOUT)
        finished = datetime.now(ZoneInfo("Asia/Shanghai")).isoformat()
        output.write(f"\n结束时间：{finished}\n退出码：{result.returncode}\n")
    with (run / "执行结果.jsonl").open("a") as output:
        output.write(json.dumps({"case": args.case, "cwd": str(repo), "command": command,
                                 "ohos": args.ohos, "started": started, "finished": finished,
                                 "exit_code": result.returncode, "log": str(log)}, ensure_ascii=False) + "\n")
    print(f"{args.case}：退出码 {result.returncode}；日志 {log}", flush=True)
    return result.returncode if result.returncode >= 0 else 128 - result.returncode


if __name__ == "__main__":
    sys.exit(main())
