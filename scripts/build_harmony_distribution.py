#!/usr/bin/env python3
"""按辅助程序签名 → 摘要编入 CLI → CLI 签名的顺序制作鸿蒙候选目录包。"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tomllib

from build_harmony import (
    CLANG_TARGET,
    REPO_ROOT,
    TARGET,
    TOOLCHAIN,
    native_sdk,
    runtime_base_contract,
)
from build_harmony_helpers import SOURCES, digest
from harmony_elf import inspect_ohos_elf
from sign_harmony import check_signature, sign_elf, signing_tool


def write_record(path: Path, value: dict) -> None:
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def source_identity() -> dict:
    def git(*args: str) -> str:
        return subprocess.check_output(["git", *args], cwd=REPO_ROOT, text=True).strip()

    patch = subprocess.check_output(["git", "diff", "--binary", "HEAD"], cwd=REPO_ROOT)
    untracked = (
        subprocess.check_output(
            ["git", "ls-files", "--others", "--exclude-standard", "-z"],
            cwd=REPO_ROOT,
        )
        .decode()
        .split("\0")
    )
    return {
        "提交": git("rev-parse", "HEAD"),
        "工作区状态": git("status", "--porcelain"),
        "工作区补丁摘要": hashlib.sha256(patch).hexdigest(),
        "新增文件摘要": {name: digest(REPO_ROOT / name) for name in untracked if name},
        "时间": datetime.now(timezone.utc).isoformat(),
    }


def run(arguments: list[str], env: dict[str, str]) -> None:
    import shlex

    print("执行：" + shlex.join(arguments), flush=True)
    subprocess.run(arguments, cwd=REPO_ROOT, env=env, check=True)


def strip_and_sign(source: Path, destination: Path, *, sdk: Path, java: str) -> dict:
    unsigned = destination.with_name(destination.name + "-未签名")
    if unsigned.exists() or destination.exists():
        raise RuntimeError(f"签名工作目录已有产物：{destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    run(
        [
            str(sdk / "llvm/bin/llvm-strip"),
            "--strip-unneeded",
            str(source),
            "-o",
            str(unsigned),
        ],
        dict(os.environ),
    )
    return sign_elf(unsigned, destination, sdk=sdk, java=java)


def build_helpers(
    args: argparse.Namespace, output: Path, sdk: Path, env: dict[str, str]
) -> None:
    identity = source_identity()
    helper_build = output / "辅助编译"
    command = [
        sys.executable,
        str(REPO_ROOT / "scripts/build_harmony_helpers.py"),
        "--sdk",
        str(sdk),
        "--output-dir",
        str(helper_build),
        "--source-cache",
        str(args.source_cache.resolve()),
    ]
    if args.offline:
        command.append("--offline")
    run(command, env)
    bwrap_build = output / "沙箱编译"
    run(
        [
            sys.executable,
            str(REPO_ROOT / "scripts/build_harmony.py"),
            "build",
            "--sdk",
            str(sdk),
            "--output-dir",
            str(bwrap_build),
            "--native-deps",
            str(helper_build / "原生依赖"),
            "--package",
            "codex-bwrap",
            "--release",
        ],
        env,
    )
    records = {}
    for name, source in {
        "rg": helper_build / "target" / TARGET / "release/rg",
        "bwrap": bwrap_build / "target" / TARGET / "release/bwrap",
    }.items():
        records[name] = strip_and_sign(
            source, output / "已签名" / name, sdk=sdk, java=args.java
        )
    licenses = output / "许可原文"
    licenses.mkdir()
    for source, name in [
        (
            helper_build / f"源码/libcap-{SOURCES['libcap'][0]}/License",
            "权限库许可.txt",
        ),
        (
            helper_build / f"源码/ripgrep-{SOURCES['ripgrep'][0]}/COPYING",
            "搜索工具许可说明.txt",
        ),
        (
            helper_build / f"源码/ripgrep-{SOURCES['ripgrep'][0]}/LICENSE-MIT",
            "搜索工具许可.txt",
        ),
        (
            helper_build / f"源码/ripgrep-{SOURCES['ripgrep'][0]}/UNLICENSE",
            "搜索工具公共领域声明.txt",
        ),
        (REPO_ROOT / "codex-rs/vendor/bubblewrap/COPYING", "沙箱工具许可.txt"),
    ]:
        shutil.copyfile(source, licenses / name)
    verify_source_unchanged(identity)
    write_record(
        output / "辅助程序记录.json",
        {
            "目标": TARGET,
            "源码": identity,
            "SDK": str(sdk),
            "上游辅助源码": SOURCES,
            "程序": records,
            "许可摘要": {p.name: digest(p) for p in licenses.iterdir()},
        },
    )


def validate_helpers(directory: Path, sdk: Path, java: str) -> dict:
    record = json.loads((directory / "辅助程序记录.json").read_text())
    if record["目标"] != TARGET:
        raise RuntimeError("辅助工具记录的目标不匹配")
    for name in ("bwrap", "rg"):
        path = directory / "已签名" / name
        if digest(path) != record["程序"][name]["输出"]["SHA-256"]:
            raise RuntimeError(f"辅助程序在签名后被更改：{name}")
        check_signature(path, sdk=sdk, java=java)
    for name, expected in record["许可摘要"].items():
        if Path(name).name != name or digest(directory / "许可原文" / name) != expected:
            raise RuntimeError(f"辅助工具许可摘要不符：{name}")
    return record


def write_checksums(directory: Path) -> None:
    lines = []
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise RuntimeError(f"候选包不接受符号链接：{path}")
        if path.is_file() and path.name != "文件校验清单.sha256":
            relative = path.relative_to(directory).as_posix()
            if any(char in relative for char in ("\n", "\r", "\\")):
                raise RuntimeError(f"文件名无法安全写入校验清单：{relative!r}")
            lines.append(f"{digest(path)}  {relative}\n")
    (directory / "文件校验清单.sha256").write_text("".join(lines))


def build_runtime_probe(
    output: Path,
    *,
    sdk: Path,
    java: str,
    env: dict[str, str],
    commit: str,
    version: str,
) -> tuple[Path, dict]:
    """Build a standalone probe that never enters Codex/arg0/config initialization."""
    source = REPO_ROOT / "scripts/harmony_runtime_probe.c"
    unsigned = output / "探针编译/harmony-runtime-probe"
    unsigned.parent.mkdir()
    run(
        [
            str(sdk / "llvm/bin/clang"),
            f"--target={CLANG_TARGET}",
            f"--sysroot={sdk / 'sysroot'}",
            "-D__MUSL__",
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-O2",
            "-fPIE",
            "-pie",
            f'-DCODEX_HARMONY_BUILD_ID="{commit}"',
            f'-DCODEX_HARMONY_VERSION="{version}"',
            str(source),
            "-ldl",
            "-o",
            str(unsigned),
        ],
        env,
    )
    signed = output / "已签名/harmony-runtime-probe"
    record = strip_and_sign(unsigned, signed, sdk=sdk, java=java)
    return signed, {
        "源码": "scripts/harmony_runtime_probe.c",
        "源码摘要": digest(source),
        "签名": record,
    }


def assemble(
    directory: Path, *, cli: Path, helpers: Path, runtime_probe: Path, version: str
) -> None:
    # Reuse the upstream layout and validation, while explicitly selecting OHOS.
    os.environ["CODEX_REPO_ROOT"] = str(REPO_ROOT)
    from codex_package.layout import build_package_dir, validate_package_dir
    from codex_package.targets import PACKAGE_VARIANTS, PackageInputs, TARGET_SPECS

    variant = PACKAGE_VARIANTS["codex"]
    spec = TARGET_SPECS[TARGET]
    directory.mkdir()
    build_package_dir(
        directory,
        version,
        variant,
        spec,
        PackageInputs(
            entrypoint_bin=cli,
            code_mode_host_bin=None,
            rg_bin=helpers / "已签名/rg",
            bwrap_bin=helpers / "已签名/bwrap",
            zsh_bin=None,
            codex_command_runner_bin=None,
            codex_windows_sandbox_setup_bin=None,
        ),
    )
    validate_package_dir(directory, variant, spec, include_zsh=False)
    if not inspect_ohos_elf(runtime_probe)["签名节存在"]:
        raise RuntimeError("目录探针缺少签名节，不能加入交付包")
    shutil.copyfile(runtime_probe, directory / "codex-resources/harmony-runtime-probe")
    (directory / "codex-resources/harmony-runtime-probe").chmod(0o755)
    shutil.copytree(helpers / "许可原文", directory / "许可原文")
    shutil.copyfile(REPO_ROOT / "LICENSE", directory / "许可原文/项目许可.txt")
    shutil.copyfile(REPO_ROOT / "NOTICE", directory / "许可原文/项目声明.txt")
    for script in ("安装.sh", "启用终端.sh", "诊断.sh"):
        shutil.copyfile(REPO_ROOT / "scripts/harmony" / script, directory / script)
        (directory / script).chmod(0o755)
    for tutorial in (
        "接口密钥安装速用.md",
        "账号登录安装速用.md",
        "升级与专项日志速用.md",
        "七版修复包安装与复测.md",
    ):
        shutil.copyfile(
            REPO_ROOT / "docs/鸿蒙电脑原生适配" / tutorial, directory / tutorial
        )
    (directory / "安装说明.md").write_text(
        "# 鸿蒙 PC 原生候选包\n\n"
        "本包为 ARM64 OHOS ELF，已用 SDK 官方工具自签名。旧包已有 PC 7.0 启动和显式 CA 后请求成功的用户反馈；"
        "本次修复包的沙箱与编码闭环仍须真机验收。\n\n"
        "解压到新目录后执行：\n\n```sh\n"
        f'sh 安装.sh --prefix "$HOME/应用工具/鸿蒙Codex-{version}"\n'
        "```\n\n按安装输出加载环境.sh，再执行 `codex --version`、`codex --help`、"
        "`codex doctor --capabilities`。在安装目录执行 `sh 启用终端.sh`，"
        "即可备份 ~/.zshrc 并更新专用启动块；重复执行不叠加，升级时更新为新版路径。"
        "撤销用 `sh 启用终端.sh --remove`。安装脚本本身不修改用户配置、不运行 Codex。\n\n"
        "默认读取鸿蒙系统 CA，仍支持 CODEX_CA_CERTIFICATE 和 SSL_CERT_FILE 覆盖。"
        "遇到目录拒绝，可先在安装目录运行 `sh 诊断.sh --paths-only`，"
        "独立原生探针直接输出身份与候选目录信息，不依赖 id，也不进入 Codex 配置初始化。"
        "运行 `sh 诊断.sh --sandbox`，"
        "将生成本机检查摘要和受限 pwd 的实际退出码；不会调用模型或导出账号配置。\n\n"
        "保留整个目录：bin/codex、codex-path/rg、codex-resources/bwrap、"
        "codex-resources/harmony-runtime-probe。"
        "任何 ELF 修改或重新签名都可能使摘要失效，必须重新制作整个包。\n\n"
        "Codex 使用原生 Shell；Git 和项目工具链由设备环境提供。"
        "本包不含 V8 代码模式宿主、定制 zsh、语音宿主或桌面自动化。"
        "受限命令执行需要设备的隔离能力，预检失败会阻止执行。"
        "账号登录、设备码、API Key 和自定义服务入口保留，本轮没有认证测试。\n\n"
        "完整说明和设备验收表见源码仓库 docs/鸿蒙电脑原生适配/。"
        "暂不使用官方普通 Linux/npm 更新包覆盖本目录。\n"
    )


def verify_source_unchanged(identity: dict) -> None:
    after = source_identity()
    if {k: v for k, v in identity.items() if k != "时间"} != {
        k: v for k, v in after.items() if k != "时间"
    }:
        raise RuntimeError("构建期间源码发生变化；请在固定源码上重新制作包")


def package_version(upstream_version: str, commit: str) -> str:
    # The upstream parser imports the package layout, which requires this root.
    os.environ["CODEX_REPO_ROOT"] = str(REPO_ROOT)
    from codex_package.cli import parse_package_version

    parse_package_version(upstream_version)
    if upstream_version.split("-", 1)[0].split("+", 1)[0] == "0.0.0":
        raise ValueError("不能发布 0.0.0 占位版本；请先核对并记录官方版本来源")
    if len(commit) != 40 or any(char not in "0123456789abcdef" for char in commit):
        raise ValueError("发布版本要求完整的小写 Git 提交 SHA")
    base, separator, metadata = upstream_version.partition("+")
    # The g prefix also keeps an all-numeric SHA with a leading zero valid SemVer.
    version = base + (".harmony.g" if "-" in base else "-harmony.g") + commit[:12]
    return parse_package_version(version + ("+" + metadata if separator else ""))


def build_package(
    args: argparse.Namespace, output: Path, sdk: Path, env: dict[str, str]
) -> None:
    upstream_version = tomllib.loads((REPO_ROOT / "codex-rs/Cargo.toml").read_text())[
        "workspace"
    ]["package"]["version"]
    version_source = json.loads(
        (REPO_ROOT / "scripts/harmony/版本来源.json").read_text()
    )
    if version_source["工作区版本"] != upstream_version:
        raise ValueError("工作区版本与版本来源记录不一致；请先核对上游 tag 和源码")
    identity = source_identity()
    version = package_version(upstream_version, identity["提交"])
    runtime_base = runtime_base_contract(args.runtime_base)
    runtime_contract = {
        "编译时固定候选": runtime_base,
        "环境变量回退": False,
        "设备验证": "待同一鸿蒙 PC 验证目录身份、权限、生命周期与隔离；不保证路径可用",
    }
    helpers = args.helpers_dir.resolve()
    helper_record = validate_helpers(helpers, sdk, args.java)
    bwrap_digest = helper_record["程序"]["bwrap"]["输出"]["SHA-256"]
    env["CODEX_BWRAP_SHA256"] = bwrap_digest
    env["CODEX_HARMONY_BUILD_ID"] = identity["提交"]
    env["STABLE_GIT_COMMIT"] = identity["提交"]
    build_dir = args.build_dir.resolve() if args.build_dir else output / "主程序编译"
    write_record(
        output / "构建输入.json",
        {
            "源码": identity,
            "版本": version,
            "版本来源": version_source,
            "沙箱摘要": bwrap_digest,
            "构建目录": str(build_dir),
            "目标": TARGET,
            "Rust": TOOLCHAIN,
            "SDK": str(sdk),
            "受保护运行根契约": runtime_contract,
        },
    )
    run(
        [
            sys.executable,
            str(REPO_ROOT / "scripts/build_harmony.py"),
            "build",
            "--sdk",
            str(sdk),
            "--output-dir",
            str(build_dir),
            "--release",
            "--runtime-base",
            runtime_base,
        ],
        env,
    )
    verify_source_unchanged(identity)
    signed_cli = output / "已签名/codex"
    cli_record = strip_and_sign(
        build_dir / "target" / TARGET / "release/codex",
        signed_cli,
        sdk=sdk,
        java=args.java,
    )
    signed_probe, probe_record = build_runtime_probe(
        output,
        sdk=sdk,
        java=args.java,
        env=env,
        commit=identity["提交"],
        version=version,
    )
    verify_source_unchanged(identity)
    directory = output / "鸿蒙Codex"
    assemble(
        directory,
        cli=signed_cli,
        helpers=helpers,
        runtime_probe=signed_probe,
        version=version,
    )
    files = {
        name: inspect_ohos_elf(directory / name)
        for name in (
            "bin/codex",
            "codex-path/rg",
            "codex-resources/bwrap",
            "codex-resources/harmony-runtime-probe",
        )
    }
    if files["codex-resources/bwrap"]["SHA-256"] != bwrap_digest:
        raise RuntimeError("打包后的 bwrap 摘要与编入 CLI 的摘要不一致")
    record = {
        "版本": version,
        "版本来源": version_source,
        "目标": TARGET,
        "源码": identity,
        "Rust": TOOLCHAIN,
        "SDK": json.loads((sdk / "oh-uni-package.json").read_text()),
        "CLI签名": cli_record,
        "目录探针": probe_record,
        "受保护运行根契约": runtime_contract,
        "辅助程序": helper_record,
        "文件": files,
        "编入CLI的沙箱摘要": bwrap_digest,
        "设备与认证验证": "未执行；候选包不代表商业 PC 适配已验收",
    }
    write_record(directory / "构建与签名记录.json", record)
    write_checksums(directory)
    archive = output / f"鸿蒙Codex-{version}-未真机验证.tar.gz"
    with tarfile.open(archive, "w:gz", compresslevel=6) as stream:
        stream.add(directory, arcname=directory.name)
    (output / "安装包校验.sha256").write_text(f"{digest(archive)}  {archive.name}\n")
    write_record(
        output / "交付摘要.json",
        {
            "归档": archive.name,
            "字节数": archive.stat().st_size,
            "SHA-256": digest(archive),
            "版本": version,
            "签名": "SDK 自签名并检查签名信息",
            "真机验收": "未执行",
        },
    )
    print(f"候选包制作完成：{archive}；商业鸿蒙 PC 运行待验收", flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("helpers", "package"))
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--java", default=shutil.which("java"))
    parser.add_argument("--source-cache", type=Path)
    parser.add_argument("--helpers-dir", type=Path)
    parser.add_argument(
        "--build-dir", type=Path, help="可选的独立编译目录；用于保留或复用构建缓存"
    )
    parser.add_argument("--offline", action="store_true")
    parser.add_argument(
        "--runtime-base",
        type=runtime_base_contract,
        help="package 必须显式绑定的目标私有目录候选；设备仍需验证",
    )
    args = parser.parse_args()
    if not args.java:
        parser.error("缺少 Java；请通过 --java 指定 SDK 签名工具使用的 Java")
    if args.action == "helpers" and (
        not args.source_cache or args.helpers_dir or args.build_dir
    ):
        parser.error(
            "helpers 要求 --source-cache，且不接受 --helpers-dir 或 --build-dir"
        )
    if args.action == "package" and (not args.helpers_dir or args.source_cache):
        parser.error("package 要求 --helpers-dir，且不接受 --source-cache")
    if args.action == "package" and args.runtime_base is None:
        parser.error("package 要求 --runtime-base，不能隐式选择运行目录")
    if args.action == "helpers" and args.runtime_base is not None:
        parser.error("helpers 不使用 --runtime-base")
    sdk = native_sdk(args.sdk)
    signing_tool(sdk)
    strip = sdk / "llvm/bin/llvm-strip"
    if not strip.is_file() or not os.access(strip, os.X_OK):
        parser.error(f"SDK 缺少可执行工具：{strip}")
    subprocess.run([args.java, "-version"], check=True)
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ)
    env["RUSTUP_AUTO_INSTALL"] = "0"
    if args.offline:
        env["CARGO_NET_OFFLINE"] = "true"
    if args.action == "helpers":
        build_helpers(args, output, sdk, env)
    else:
        build_package(args, output, sdk, env)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
