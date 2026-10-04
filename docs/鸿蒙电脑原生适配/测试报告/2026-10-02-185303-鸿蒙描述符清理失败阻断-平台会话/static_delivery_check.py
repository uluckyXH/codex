#!/usr/bin/env python3
"""记录本批环境与 OHOS 测试 ELF 的静态属性，不执行目标程序。"""

import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

REPORT = Path(__file__).resolve().parent
REPO = REPORT.parents[3]
RUN = REPO / ".harmony-build/测试批次" / REPORT.name
SDK = Path("/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native")


def run(command):
    print("命令：" + json.dumps([str(value) for value in command], ensure_ascii=False), flush=True)
    output = subprocess.check_output([str(value) for value in command], text=True, cwd=REPO)
    print(output, end="" if output.endswith("\n") else "\n")
    return output.strip()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    environment = {
        "系统": run(["sw_vers"]),
        "架构": run(["uname", "-m"]),
        "Rust": run(["rustup", "run", "1.95.0", "rustc", "--version"]),
        "Cargo": run(["rustup", "run", "1.95.0", "cargo", "--version"]),
        "Python": sys.version,
        "Clang": run([SDK / "llvm/bin/clang", "--version"]),
        "链接器": run([SDK / "llvm/bin/ld.lld", "--version"]),
        "SDK": json.loads((SDK / "oh-uni-package.json").read_text()),
    }
    (REPORT / "环境摘要.json").write_text(json.dumps(environment, ensure_ascii=False, indent=2) + "\n")
    log = RUN / "日志/交叉03-描述符目标测试链接.log"
    paths = re.findall(r"Executable .*? \((.+?)\)", log.read_text())
    if len(paths) != 3:
        raise RuntimeError(f"期望 3 个目标测试 ELF，实际日志给出 {len(paths)} 个")
    artifacts = []
    for relative in paths:
        path = REPO / relative
        if not path.is_relative_to(RUN):
            raise RuntimeError("产物不在独立批次目录")
        info = run([SDK / "llvm/bin/llvm-readelf", "-h", "-l", "-d", path])
        if "AArch64" not in info or "ld-musl-aarch64.so.1" not in info:
            raise RuntimeError("ELF 架构或加载器不符合目标")
        artifacts.append({
            "文件": relative,
            "SHA256": digest(path),
            "字节数": path.stat().st_size,
            "架构": "ELF64 / AArch64",
            "解释器": re.findall(r"Requesting program interpreter: ([^\]]+)", info),
            "动态依赖": re.findall(r"NEEDED.*?\[([^\]]+)\]", info),
            "RPATH或RUNPATH": [line.strip() for line in info.splitlines() if "RPATH" in line or "RUNPATH" in line],
            "实际运行": False,
        })
    (REPORT / "产物摘要.json").write_text(json.dumps(artifacts, ensure_ascii=False, indent=2) + "\n")
    identity = json.loads((REPORT / "源码身份.json").read_text())
    for relative, before in identity["文件SHA256"].items():
        if digest(REPO / relative) != before:
            raise RuntimeError(f"测试期间源码发生变化：{relative}")
    # 确认目标条件用例实际进入链接产物，只检查符号，不运行测试。
    main_elf = REPO / next(path for path in paths if "/codex_utils_pty-" in path)
    symbols = subprocess.check_output(
        [str(SDK / "llvm/bin/llvm-nm"), "--demangle", "--defined-only", str(main_elf)],
        text=True,
    )
    symbol = "cleanup_failures_prevent_explicit_launch_on_ohos"
    selected = [line for line in symbols.splitlines() if symbol in line]
    if not selected:
        raise RuntimeError("目标 ELF 中未找到 OHOS 清理失败用例")
    print("OHOS 用例符号：" + "\n".join(selected))
    print("源码摘要一致；三个目标测试 ELF 的静态属性检查通过；未运行目标程序。")


if __name__ == "__main__":
    main()
