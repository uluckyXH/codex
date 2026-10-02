#!/usr/bin/env python3
"""从固定且校验过的源码构建 OHOS 原生 libcap、rg；不执行目标程序。"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
from urllib.request import urlopen

from build_harmony import TARGET, TOOLCHAIN, build_environment, native_sdk


SOURCES = {
    "libcap": (
        "2.78",
        "https://cdn.kernel.org/pub/linux/libs/security/linux-privs/libcap2/libcap-2.78.tar.xz",
        "0d621e562fd932ccf67b9660fb018e468a683d7b827541df27813228c996bb11",
    ),
    "ripgrep": (
        "15.2.0",
        "https://static.crates.io/crates/ripgrep/ripgrep-15.2.0.crate",
        "a30750b6d0743bfdd2656ebbaf4555aa278c43144b84bc389bcbfa399485ec71",
    ),
}


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def source_archive(name: str, cache: Path, *, offline: bool) -> Path:
    _, url, expected = SOURCES[name]
    path = cache / url.rsplit("/", 1)[-1]
    cache.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        if offline:
            raise RuntimeError(f"离线源码缓存缺失：{path}")
        temporary = path.with_suffix(path.suffix + ".download")
        try:
            with urlopen(url, timeout=120) as response, temporary.open("wb") as out:
                shutil.copyfileobj(response, out)
            if digest(temporary) != expected:
                raise RuntimeError(f"下载源码的 SHA-256 不匹配：{name}")
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)
    if digest(path) != expected:
        raise RuntimeError(f"缓存源码的 SHA-256 不匹配：{path}")
    return path


def unpack(name: str, archive: Path, destination: Path) -> Path:
    version, _, _ = SOURCES[name]
    source = destination / f"{name}-{version}"
    if source.exists():
        raise RuntimeError(f"源码工作目录已存在，请选择新的 --output-dir：{source}")
    destination.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive) as package:
        # Never extract links or paths outside this exact version's source tree.
        for member in package.getmembers():
            parts = Path(member.name).parts
            if not parts or parts[0] != source.name or ".." in parts:
                raise RuntimeError(f"源码归档含非法路径：{member.name}")
        package.extractall(destination, filter="data")
    return source


def run(command: list[str], *, cwd: Path, env: dict[str, str]) -> None:
    import shlex

    print("执行：" + shlex.join(command), flush=True)
    subprocess.run(command, cwd=cwd, env=env, check=True)


def build_libcap(source: Path, output: Path, sdk: Path, env: dict[str, str]) -> Path:
    library = source / "libcap"
    # This is the upstream Makefile's name-list generation, expressed portably
    # rather than relying on GNU sed's \s extension on a macOS host.
    header = (library / "include/uapi/linux/capability.h").read_text()
    names = re.findall(r"^#define\s+(CAP_\w+)\s+(\d+)\s*$", header, re.M)
    if not names:
        raise RuntimeError("libcap 源码没有可识别的 capability 定义")
    (library / "cap_names.list.h").write_text(
        "".join(f'{{"{name.lower()}",{value}}},\n' for name, value in names)
    )
    host_cc = shutil.which("cc", path=env.get("PATH"))
    if host_cc is None:
        raise RuntimeError("缺少宿主 C 编译器，无法生成 libcap 名称表")
    generator = library / "_makenames"
    run([host_cc, "-O2", "_makenames.c", "-o", str(generator)], cwd=library, env=env)
    with (library / "cap_names.h").open("w") as out:
        subprocess.run([str(generator)], cwd=library, env=env, stdout=out, check=True)
    objects = []
    for name in [
        "cap_alloc",
        "cap_proc",
        "cap_extint",
        "cap_flag",
        "cap_text",
        "cap_file",
        "cap_syscalls",
    ]:
        obj = library / f"{name}.o"
        run(
            [
                env[f"CC_{TARGET}"],
                "-O2",
                "-fPIC",
                "-D_LARGEFILE64_SOURCE",
                "-D_FILE_OFFSET_BITS=64",
                "-D_LIBPSX_PTHREAD_LINKAGE",
                f"-I{library / 'include'}",
                f"-I{library / 'include/uapi'}",
                "-c",
                str(library / f"{name}.c"),
                "-o",
                str(obj),
            ],
            cwd=library,
            env=env,
        )
        objects.append(obj)
    prefix = output / "原生依赖"
    (prefix / "lib/pkgconfig").mkdir(parents=True)
    (prefix / "include/sys").mkdir(parents=True)
    archive = prefix / "lib/libcap.a"
    run(
        [str(sdk / "llvm/bin/llvm-ar"), "rcs", str(archive), *map(str, objects)],
        cwd=library,
        env=env,
    )
    run([str(sdk / "llvm/bin/llvm-ranlib"), str(archive)], cwd=library, env=env)
    shutil.copyfile(
        library / "include/sys/capability.h", prefix / "include/sys/capability.h"
    )
    (prefix / "lib/pkgconfig/libcap.pc").write_text(
        "prefix=/\nlibdir=${prefix}lib\nincludedir=${prefix}include\n"
        f"Name: libcap\nDescription: OHOS static libcap\nVersion: {SOURCES['libcap'][0]}\n"
        "Libs: -L${libdir} -lcap\nCflags: -I${includedir}\n"
    )
    return archive


def build_ripgrep(source: Path, output: Path, env: dict[str, str]) -> Path:
    rustup = shutil.which("rustup", path=env.get("PATH"))
    if rustup is None:
        raise RuntimeError("缺少 rustup")
    run(
        [
            rustup,
            "run",
            TOOLCHAIN,
            "cargo",
            "build",
            "--locked",
            "--release",
            "--target",
            TARGET,
            "--bin",
            "rg",
        ],
        cwd=source,
        env=env,
    )
    return output / "target" / TARGET / "release/rg"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--source-cache", type=Path, required=True)
    parser.add_argument("--only", choices=SOURCES)
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    sdk = native_sdk(args.sdk)
    output = args.output_dir.resolve()
    env = build_environment(sdk, output, dict(os.environ))
    env["RUSTUP_AUTO_INSTALL"] = "0"
    if args.offline:
        env["CARGO_NET_OFFLINE"] = "true"
    artifacts = {}
    for name in [args.only] if args.only else SOURCES:
        archive = source_archive(
            name, args.source_cache.resolve(), offline=args.offline
        )
        source = unpack(name, archive, output / "源码")
        artifact = (
            build_libcap(source, output, sdk, env)
            if name == "libcap"
            else build_ripgrep(source, output, env)
        )
        artifacts[name] = {
            "版本": SOURCES[name][0],
            "源码地址": SOURCES[name][1],
            "源码摘要": SOURCES[name][2],
            "产物": str(artifact),
            "SHA-256": digest(artifact),
        }
    (output / "辅助工具构建记录.json").write_text(
        json.dumps(
            {
                "目标": TARGET,
                "工具链": TOOLCHAIN,
                "产物": artifacts,
                "签名与执行": "未签名，未执行；交叉构建不代表商业鸿蒙 PC 功能通过",
            },
            ensure_ascii=False,
            indent=2,
        )
        + "\n"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
