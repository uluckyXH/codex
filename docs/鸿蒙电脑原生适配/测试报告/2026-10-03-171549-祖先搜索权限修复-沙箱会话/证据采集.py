#!/usr/bin/env python3
"""收集本批已有测试产物的静态证据；不执行目标 ELF。"""

from datetime import datetime
import hashlib
import json
from pathlib import Path
import platform
import shlex
import subprocess
from zoneinfo import ZoneInfo


REPORT = Path(__file__).resolve().parent
REPO = REPORT.parents[3]
RUN = REPO / ".harmony-build/测试批次" / REPORT.name
SDK = Path("/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native")
SOURCE_COMMIT = "51dd76fc8d932a8cf402cbe892380787a9b1a8b7"
LIBC = Path("/Volumes/MacSSD/dev/rust/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libc-0.2.186/src")
STD = Path("/Volumes/MacSSD/dev/rust/rustup/toolchains/1.95.0-aarch64-apple-darwin/lib/rustlib/src/rust/library/std/src")


def sha(data):
    return hashlib.sha256(data).hexdigest()


def command(args):
    args = [str(arg) for arg in args]
    result = subprocess.run(args, cwd=REPO, capture_output=True, text=True)
    print(f"命令：{shlex.join(args)}\n退出码：{result.returncode}", flush=True)
    if result.returncode:
        print(result.stdout + result.stderr, flush=True)
        result.check_returncode()
    return result.stdout + result.stderr


def save(name, text):
    with (REPORT / name).open("x") as output:
        output.write("\n".join(line.rstrip() for line in text.splitlines()) + "\n")


def save_json(name, value):
    save(name, json.dumps(value, ensure_ascii=False, indent=2))


def main():
    now = datetime.now(ZoneInfo("Asia/Shanghai")).isoformat()
    environment = [f"实际采集时间：{now}", f"工作目录：{REPO}", f"Python：{platform.python_version()}"]
    for args in (["sw_vers"], ["uname", "-m"], ["id", "-u"],
                 ["rustup", "run", "1.95.0", "rustc", "--version", "--verbose"],
                 ["rustup", "run", "1.95.0", "cargo", "--version"],
                 [SDK / "llvm/bin/clang", "--version"], [SDK / "llvm/bin/ld.lld", "--version"]):
        environment.extend((f"命令：{shlex.join([str(arg) for arg in args])}", command(args)))
    environment.extend(("SDK metadata：", (SDK / "oh-uni-package.json").read_text(),
                        "宿主目标：aarch64-apple-darwin；交叉目标：aarch64-unknown-linux-ohos",
                        "Cargo：--locked --offline；默认 features；dev/test profile；CARGO_BUILD_JOBS=2",
                        "Rust/Cargo 缓存固定于外置盘；完整子进程环境见验证入口.py，仅采集任务相关信息。",
                        "目标构建候选契约：CODEX_OHOS_RUNTIME_BASE=/data/storage/el2/base/files",
                        f"输出/日志：{RUN}",
                        "普通临时目录为本批外置盘 rt-*；严格祖先及短 socket 夹具为少量 /private/tmp/cr*、cs*，由 TempDir 清理。",
                        "宿主 socket 测试在执行沙箱许可后实际运行；无认证、凭据、登录或模型请求测试。",
                        "没有目标设备、签名、安装或完整 CLI release。"))
    save("环境摘要.txt", "\n".join(environment))

    excerpts = []
    for path, start, end in (
        (SDK / "sysroot/usr/include/aarch64-linux-ohos/bits/fcntl.h", 1, 25),
        (SDK / "sysroot/usr/include/fcntl.h", 33, 59),
        (SDK / "sysroot/usr/include/sys/stat.h", 63, 79),
        (LIBC / "unix/linux_like/linux/musl/mod.rs", 570, 576),
        (LIBC / "unix/linux_like/linux/mod.rs", 4421, 4432),
        (STD / "sys/fs/unix.rs", 1343, 1380),
        (STD / "sys/fd/unix.rs", 628, 638),
    ):
        lines = path.read_text().splitlines()
        excerpts.extend((f"来源：{path}", f"文件 SHA256：{sha(path.read_bytes())}"))
        excerpts.extend(f"{index + 1}: {lines[index]}" for index in range(start - 1, min(end, len(lines))))
        excerpts.append("")
    save("SDK与标准库证据.txt", "\n".join(excerpts))

    binaries = [path for path in (RUN / "鸿蒙构建/target/aarch64-unknown-linux-ohos/debug/deps").glob("codex_uds-*")
                if path.suffix == "" and path.read_bytes()[:4] == b"\x7fELF"]
    assert len(binaries) == 1, binaries
    binary = binaries[0]
    elf = command([SDK / "llvm/bin/llvm-readelf", "-h", "-l", "-d", binary])
    assert "AArch64" in elf and "/lib/ld-musl-aarch64.so.1" in elf
    assert "[libc.so]" in elf and "RPATH" not in elf and "RUNPATH" not in elf
    symbol_output = command([SDK / "llvm/bin/llvm-nm", "--demangle", binary])
    test_names = ("new_private_directory_keeps_a_chmod_capable_descriptor",
                  "search_only_ancestor_supports_path_handles_without_weakening_validation")
    symbols = [line for line in symbol_output.splitlines() if any(name in line for name in test_names)]
    assert all(any(name in line for line in symbols) for name in test_names)
    save("ELF静态检查.txt", f"文件：{binary}\nSHA256：{sha(binary.read_bytes())}\n未执行目标程序。\n\n{elf}\n目标新增测试符号：\n" + "\n".join(symbols))
    save_json("目标产物摘要.json", {"path": str(binary), "sha256": sha(binary.read_bytes()),
                                  "size": binary.stat().st_size, "executed": False})

    snapshot = json.loads((REPORT / "测试源码快照.json").read_text())
    matches = []
    for name, expected in snapshot["files"].items():
        committed = subprocess.run(["git", "show", f"{SOURCE_COMMIT}:{name}"], cwd=REPO,
                                   capture_output=True, check=True).stdout
        matches.append({"file": name, "snapshot": expected, "commit": sha(committed),
                        "current": sha((REPO / name).read_bytes())})
    assert all(item["snapshot"] == item["commit"] == item["current"] for item in matches)
    save_json("源码与提交核对.json", {"checked_at": now, "tested_head": snapshot["HEAD"],
                                  "source_commit": SOURCE_COMMIT, "all_match": True, "files": matches})
    inputs = {str(path.relative_to(REPO)): sha(path.read_bytes()) for path in (
        REPO / "codex-rs/Cargo.toml", REPO / "codex-rs/Cargo.lock", REPO / "scripts/build_harmony.py")}
    wrappers = {path.name: {"sha256": sha(path.read_bytes()), "content": path.read_text()}
                for path in sorted((RUN / "鸿蒙构建/toolchain").iterdir()) if path.is_file()}
    save_json("构建入口与包装器.json", {"inputs": inputs, "wrappers": wrappers})
    print("环境、SDK 常量/标准库路径、测试 ELF 与源码摘要采集通过；没有执行目标 ELF。", flush=True)


if __name__ == "__main__":
    main()
