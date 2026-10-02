#!/usr/bin/env python3
"""直接检查原始 Shell 源文件的 OHOS 分支；不代表完整包通过检查。"""

import json
import os
from pathlib import Path
import shlex
import subprocess
import sys


repo = Path(__file__).resolve().parents[4]
target = "aarch64-unknown-linux-ohos"
output = Path(os.environ["CARGO_TARGET_DIR"])
command = [
    "rustup", "run", "1.95.0", "cargo", "check",
    "--manifest-path", str(repo / "codex-rs/Cargo.toml"),
    "--locked", "--target", target,
    "-p", "codex-install-context", "-p", "which@8.0.0",
    "--message-format=json",
]
print("依赖检查命令：" + shlex.join(command), flush=True)
artifacts = {}
with subprocess.Popen(command, cwd=repo, stdout=subprocess.PIPE, text=True) as child:
    for line in child.stdout:
        message = json.loads(line)
        if message.get("reason") == "compiler-message":
            print(message["message"].get("rendered", line), end="", flush=True)
        if message.get("reason") == "compiler-artifact":
            name = message["target"]["name"]
            if name in ("serde", "which"):
                artifacts[name] = next(Path(path) for path in message["filenames"] if path.endswith(".rmeta"))
    result = child.wait()
if result:
    sys.exit(result)
command = [
    "rustup", "run", "1.95.0", "rustc", "--edition=2024",
    "--crate-name", "codex_harmony_shell_probe", "--crate-type", "lib",
    "--target", target, "--emit=metadata",
    "-L", f"dependency={output / target / 'debug/deps'}",
    "-L", f"dependency={output / 'debug/deps'}",
    "--extern", f"serde={artifacts['serde']}",
    "--extern", f"which={artifacts['which']}",
    str(repo / "codex-rs/shell-command/src/shell_detect.rs"),
    "-o", str(output.parent / "Shell目标源码检查.rmeta"),
]
print("原始源文件检查命令：" + shlex.join(command), flush=True)
subprocess.run(command, cwd=repo, check=True)
print("OHOS 原始 Shell 源文件元数据检查通过；没有完成 codex-shell-command 包级检查或链接。")
