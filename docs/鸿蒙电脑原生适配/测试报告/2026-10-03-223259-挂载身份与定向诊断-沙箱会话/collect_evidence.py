#!/usr/bin/env python3
"""读取本批环境、源码及已链接测试 ELF；不执行目标程序。"""

from datetime import datetime
import hashlib
import json
from pathlib import Path
import platform
import shlex
import subprocess
import sys
from zoneinfo import ZoneInfo

REPORT = Path(__file__).resolve().parent
REPO = REPORT.parents[3]
RUN = REPO / ".harmony-build/测试批次" / REPORT.name
SDK = Path("/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native")
REGISTRY = Path("/Volumes/MacSSD/dev/rust/cargo/registry/src/index.crates.io-1949cf8c6b5b557f")
SOURCE_COMMIT = "99fde6f792f66dcb1d48c1413d66e403e3c6ccb7"


def digest(path):
    with path.open("rb") as data:
        return hashlib.file_digest(data, "sha256").hexdigest()


def save(name, content):
    if not isinstance(content, str):
        content = json.dumps(content, ensure_ascii=False, indent=2)
    with (REPORT / name).open("x") as output:
        output.write("\n".join(line.rstrip() for line in content.splitlines()).rstrip() + "\n")


def command(arguments):
    arguments = [str(arg) for arg in arguments]
    print("命令：" + shlex.join(arguments), flush=True)
    result = subprocess.run(arguments, cwd=REPO, text=True, capture_output=True)
    print(f"退出码：{result.returncode}", flush=True)
    if result.returncode:
        print(result.stdout + result.stderr, flush=True)
        result.check_returncode()
    return result.stdout + result.stderr


def environment():
    records = [f"采样时间：{datetime.now(ZoneInfo('Asia/Shanghai')).isoformat()}",
               f"工作目录：{REPO}", f"Python：{platform.python_version()}"]
    for args in (["sw_vers"], ["uname", "-m"], ["id", "-u"],
                 ["rustup", "run", "1.95.0", "rustc", "--version", "--verbose"],
                 ["rustup", "run", "1.95.0", "cargo", "--version"],
                 [SDK / "llvm/bin/clang", "--version"], [SDK / "llvm/bin/ld.lld", "--version"]):
        records.extend((shlex.join([str(arg) for arg in args]), command(args)))
    records += ["SDK metadata：", (SDK / "oh-uni-package.json").read_text(),
                "宿主目标：aarch64-apple-darwin；交叉目标：aarch64-unknown-linux-ohos",
                "默认features；dev/test profile；--locked --offline；jobs=2。任务指定环境见验证入口.py。",
                "目标编译契约：CODEX_OHOS_RUNTIME_BASE=/data/storage/el2/base/files。",
                "本批没有设置 CODEX_HARMONY_BUILD_ID，不作为发布候选身份。",
                f"构建、临时文件和日志：{RUN}",
                "仅宿主运行合成挂载表及逻辑测试；无设备、签名、安装、认证、登录或模型测试。"]
    save("环境摘要.txt", "\n".join(records))
    excerpts = []
    for path, start, end in (
        (SDK / "sysroot/usr/include/linux/stat.h", 54, 100),
        (SDK / "sysroot/usr/include/sys/sysmacros.h", 1, 45),
        (REGISTRY / "rustix-1.1.4/src/fs/statx.rs", 14, 51),
        (REGISTRY / "rustix-1.1.4/src/backend/linux_raw/fs/syscalls.rs", 818, 853),
        (REGISTRY / "rustix-1.1.4/src/backend/libc/fs/syscalls.rs", 2035, 2053),
    ):
        excerpts += [f"来源：{path}", f"SHA256：{digest(path)}"]
        lines = path.read_text().splitlines()
        excerpts += [f"{i + 1}: {lines[i]}" for i in range(start - 1, min(end, len(lines)))]
        excerpts += [""]
    save("SDK与Rust接口摘录.txt", "\n".join(excerpts))
    snapshot = json.loads((REPORT / "测试源码快照.json").read_text())
    files = []
    for name, expected in snapshot["files"].items():
        content = subprocess.run(["git", "show", f"{SOURCE_COMMIT}:{name}"], cwd=REPO,
                                 check=True, capture_output=True).stdout
        files.append({"file": name, "snapshot": expected, "commit": hashlib.sha256(content).hexdigest(),
                      "current": digest(REPO / name)})
    assert all(item["snapshot"] == item["commit"] == item["current"] for item in files)
    save("源码与提交核对.json", {"tested_head": snapshot["HEAD"], "source_commit": SOURCE_COMMIT,
                               "all_match": True, "files": files})
    wrappers = {path.name: {"sha256": digest(path), "content": path.read_text()}
                for path in sorted((RUN / "鸿蒙构建/toolchain").iterdir()) if path.is_file()}
    configurations = {}
    for path in sorted((RUN / "鸿蒙构建/target/aarch64-unknown-linux-ohos/debug/build").glob("rustix-*/output")):
        configurations[str(path.relative_to(RUN))] = [line for line in path.read_text().splitlines()
                                                    if line.startswith("cargo:rustc-cfg=")]
    save("编译包装器与Rustix后端.json", {"wrappers": wrappers, "rustix_cfg": configurations})
    print("环境与源码证据采集通过；没有执行目标程序。", flush=True)


def elf():
    binaries = []
    for path in (RUN / "鸿蒙构建/target/aarch64-unknown-linux-ohos/debug/deps").glob("codex_linux_sandbox-*"):
        if path.suffix == "" and path.is_file():
            with path.open("rb") as data:
                if data.read(4) == b"\x7fELF":
                    binaries.append(path)
    assert len(binaries) == 1, binaries
    binary = binaries[0]
    information = command([SDK / "llvm/bin/llvm-readelf", "-h", "-l", "-d", binary])
    assert "AArch64" in information and "/lib/ld-musl-aarch64.so.1" in information
    symbols = command([SDK / "llvm/bin/llvm-nm", "--demangle", "--defined-only", binary])
    names = ("synthetic_ohos_statx_identity_resolves_device_encoding_without_losing_aliases",
             "diagnostics_are_bounded_lossless_and_cannot_inject_log_lines")
    selected = [line for line in symbols.splitlines() if any(name in line for name in names)]
    assert all(any(name in line for line in selected) for name in names)
    summary = {"path": str(binary), "sha256": digest(binary), "size": binary.stat().st_size,
               "executed": False, "signed": False}
    save("目标产物摘要.json", summary)
    save("ELF静态检查.txt", f"文件：{binary}\nSHA256：{summary['sha256']}\n未执行目标测试。\n\n"
         + information + "\n新增测试符号：\n" + "\n".join(selected))
    print("目标 ELF 静态检查完成，未执行测试程序。", flush=True)


if __name__ == "__main__":
    {"environment": environment, "elf": elf}[sys.argv[1]]()
