#!/usr/bin/env python3
"""使用指定的鸿蒙 SDK 构建 ARM64 候选产物；不签名、不运行目标程序。"""

import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tomllib


REPO_ROOT = Path(__file__).resolve().parents[1]
TARGET = "aarch64-unknown-linux-ohos"
CLANG_TARGET = "aarch64-linux-ohos"
# 跟随仓库的上游版本锁，避免每次合并 Rust 升级时维护第二份版本号。
TOOLCHAIN = tomllib.loads((REPO_ROOT / "codex-rs/rust-toolchain.toml").read_text())[
    "toolchain"
]["channel"]


def native_sdk(path: Path) -> Path:
    """接受 native 目录、openharmony 目录或 DevEco 的 SDK 版本目录。"""
    for candidate in (path, path / "native", path / "openharmony/native"):
        if (candidate / "sysroot/usr/include/spawn.h").is_file():
            root = candidate.resolve()
            for name in ("clang", "clang++", "llvm-ar", "llvm-readelf"):
                tool = root / "llvm/bin" / name
                if not tool.is_file() or not os.access(tool, os.X_OK):
                    raise ValueError(f"SDK 缺少可执行工具：{tool}")
            if not (root / "sysroot/usr/lib" / CLANG_TARGET / "libc.so").is_file():
                raise ValueError(f"SDK 缺少 {CLANG_TARGET} 的 libc.so：{root}")
            return root
    raise ValueError(f"找不到原生 SDK 的 sysroot：{path}")


def write_wrapper(path: Path, compiler: Path, sdk: Path) -> None:
    command = [
        str(compiler),
        f"--target={CLANG_TARGET}",
        f"--sysroot={sdk / 'sysroot'}",
        "-D__MUSL__",
    ]
    path.write_text("#!/bin/sh\nexec " + shlex.join(command) + ' "$@"\n')
    path.chmod(0o755)


def build_environment(sdk: Path, output: Path, base: dict[str, str]) -> dict[str, str]:
    env = base.copy()
    wrappers = output / "toolchain"
    wrappers.mkdir(parents=True, exist_ok=True)
    cc = wrappers / "ohos-clang"
    cxx = wrappers / "ohos-clang++"
    write_wrapper(cc, sdk / "llvm/bin/clang", sdk)
    write_wrapper(cxx, sdk / "llvm/bin/clang++", sdk)
    # 只设置目标专用变量，宿主的 build.rs、过程宏仍使用宿主编译器。
    env[f"CARGO_TARGET_{TARGET.replace('-', '_').upper()}_LINKER"] = str(cc)
    for prefix, value in (("CC", cc), ("CXX", cxx), ("AR", sdk / "llvm/bin/llvm-ar")):
        for suffix in (TARGET, TARGET.replace("-", "_")):
            env[f"{prefix}_{suffix}"] = str(value)
    # aws-lc 等依赖把 OHOS_NDK_HOME 解释为含 native 的目录。
    # 向子进程统一传入规范路径，避免拼出 native/native。
    env["OHOS_NDK_HOME"] = str(sdk.parent)
    env["OHOS_SDK_NATIVE"] = str(sdk)
    cmake_bin = sdk / "build-tools/cmake/bin"
    if (cmake_bin / "cmake").is_file():
        env["PATH"] = str(cmake_bin) + os.pathsep + env.get("PATH", os.defpath)
        for suffix in (TARGET, TARGET.replace("-", "_")):
            env[f"CMAKE_{suffix}"] = str(cmake_bin / "cmake")
    toolchain = sdk / "build/cmake/ohos.toolchain.cmake"
    if toolchain.is_file():
        for suffix in (TARGET, TARGET.replace("-", "_")):
            env[f"CMAKE_TOOLCHAIN_FILE_{suffix}"] = str(toolchain)
    # 不允许 pkg-config 为目标引入 Homebrew / 系统宿主库。保留调用者
    # 明确配置的目标依赖目录，未配置时只搜索 SDK 的目标目录。
    suffix = TARGET.replace("-", "_")
    defaults = {
        "PKG_CONFIG_SYSROOT_DIR": str(sdk / "sysroot"),
        "PKG_CONFIG_LIBDIR": str(sdk / "sysroot/usr/lib" / CLANG_TARGET / "pkgconfig"),
        "PKG_CONFIG_PATH": "",
    }
    for prefix, value in defaults.items():
        selected = env.get(f"{prefix}_{TARGET}", env.get(f"{prefix}_{suffix}", value))
        env[f"{prefix}_{TARGET}"] = selected
        env[f"{prefix}_{suffix}"] = selected
    env["CARGO_TARGET_DIR"] = str(output / "target")
    # rustc、C/C++ 和依赖构建脚本的临时文件也跟随输出目录。
    temporary = output / "tmp"
    temporary.mkdir(exist_ok=True)
    env["TMPDIR"] = str(temporary)
    return env


def run(command: list[str], env: dict[str, str]) -> None:
    print("执行：" + shlex.join(command), flush=True)
    subprocess.run(command, cwd=REPO_ROOT / "codex-rs", env=env, check=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("probe", "check", "build"))
    parser.add_argument("--sdk", type=Path, default=os.environ.get("OHOS_NDK_HOME"))
    parser.add_argument("--output-dir", type=Path, default=REPO_ROOT / ".harmony-build")
    parser.add_argument(
        "--package", action="append", help="check/build 的 Cargo 包名，可重复"
    )
    parser.add_argument(
        "--release", action="store_true", help="check/build 使用 release 配置"
    )
    args = parser.parse_args()
    try:
        if args.sdk is None:
            raise ValueError("请提供 --sdk 或设置 OHOS_NDK_HOME，指向鸿蒙原生 SDK")
        if args.action == "probe" and (args.package or args.release):
            raise ValueError("probe 不接受 --package 或 --release")
        sdk = native_sdk(args.sdk.expanduser())
        rustup = shutil.which("rustup")
        if rustup is None:
            raise ValueError("找不到 rustup；请先按中文构建环境文档安装并配置 PATH")
        output = args.output_dir.expanduser().resolve()
        env = build_environment(sdk, output, dict(os.environ))
        rust = [rustup, "run", TOOLCHAIN]
        # 缺少工具链或目标时停止，避免在未配置的内部磁盘位置自动安装。
        env["RUSTUP_AUTO_INSTALL"] = "0"
        run([*rust, "rustc", "--version"], env)
        run([*rust, "rustc", "--print", "cfg", "--target", TARGET], env)
        metadata = sdk / "oh-uni-package.json"
        sdk_info = json.loads(metadata.read_text()) if metadata.exists() else {}
        print("SDK：" + json.dumps(sdk_info, ensure_ascii=False), flush=True)
        if args.action == "probe":
            binary = output / "harmony-rust-probe"
            run(
                [
                    *rust,
                    "rustc",
                    "--edition=2024",
                    "--target",
                    TARGET,
                    "-C",
                    f"linker={output / 'toolchain/ohos-clang'}",
                    str(REPO_ROOT / "scripts/harmony_probe.rs"),
                    "-o",
                    str(binary),
                ],
                env,
            )
            run(
                [str(sdk / "llvm/bin/llvm-readelf"), "-h", "-l", "-d", str(binary)], env
            )
        else:
            command = [*rust, "cargo", args.action, "--locked", "--target", TARGET]
            for package in args.package or ["codex-cli"]:
                command.extend(["--package", package])
            if args.release:
                command.append("--release")
            run(command, env)
        print(f"{args.action} 成功；产物与缓存目录：{output}")
        print("此结果不代表已签名或通过鸿蒙 PC 真机验证。")
        return 0
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"构建失败：{error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
