#!/usr/bin/env python3
"""按辅助程序签名 → 摘要编入 CLI → CLI 签名的顺序制作鸿蒙候选目录包。"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tomllib

from build_harmony import (
    CLANG_TARGET,
    REPO_ROOT,
    RUNTIME_PROFILES,
    TARGET,
    TOOLCHAIN,
    hnp_alias_build_environment,
    native_sdk,
    runtime_base_contract,
    runtime_build_environment,
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
    unsigned = destination.with_name(destination.name + "-unsigned")
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
    runtime_base: str | None,
    runtime_profile: str = "platform",
) -> tuple[Path, dict]:
    """Build a standalone probe that never enters Codex/arg0/config initialization."""
    source = REPO_ROOT / "scripts/harmony_runtime_probe.c"
    unsigned = output / "探针编译/harmony-runtime-probe"
    unsigned.parent.mkdir()
    # Fail the package build if a different SDK changes the dynamically loaded
    # directory APIs. This translation unit is never linked into the probe.
    run(
        [
            str(sdk / "llvm/bin/clang"),
            f"--target={CLANG_TARGET}",
            f"--sysroot={sdk / 'sysroot'}",
            "-D__MUSL__",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-x",
            "c++",
            "-std=c++17",
            "-fsyntax-only",
            str(REPO_ROOT / "scripts/harmony_runtime_probe_sdk_check.cpp"),
        ],
        env,
    )
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
            *(["-DCODEX_OHOS_PLATFORM_DATA=1"] if runtime_profile == "platform" else []),
            *(
                [
                    "-DCODEX_OHOS_RUNTIME_BASE="
                    + json.dumps(runtime_base_contract(runtime_base), ensure_ascii=False)
                ]
                if runtime_base is not None
                else []
            ),
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
        "编译时运行根": runtime_base,
        "主程序运行目录策略": runtime_profile,
        "观察边界": "独立进程只读候选与 Context 观察，不批准运行根，不初始化 CLI 数据目录",
        "旧编译根布局观察": runtime_base is not None,
        "通用目录观察": runtime_profile == "platform",
        "SDK接口类型检查": "三个目录 API 类型均与本次 SDK 声明一致；仅编译检查",
        "相关源码摘要": {
            name: digest(REPO_ROOT / "scripts" / name)
            for name in (
                "harmony_runtime_probe.c",
                "harmony_runtime_probe_context.h",
                "harmony_runtime_probe_sdk_check.cpp",
            )
        },
        "签名": record,
    }


HNP_ALIAS_NAMES = (
    "apply_patch",
    "applypatch",
    "codex-linux-sandbox",
    "codex-execve-wrapper",
)

INSTALL_TUTORIALS = (
    "新版安装与运行说明.md",
    "接口密钥安装速用.md",
    "账号登录安装速用.md",
)


def render_install_tutorials(
    directory: Path, *, version: str, source_commit: str, runtime_profile: str
) -> None:
    if not re.fullmatch(r"[0-9a-f]{40}", source_commit):
        raise ValueError("安装文档要求完整的小写 Git 提交 SHA")
    values = {
        "PACKAGE_VERSION": version,
        "SOURCE_COMMIT": source_commit,
        "RUNTIME_PROFILE": runtime_profile,
        "ARCHIVE_NAME": f"鸿蒙Codex-{version}-未真机验证.tar.gz",
    }
    for name in INSTALL_TUTORIALS:
        content = (REPO_ROOT / "docs/鸿蒙电脑原生适配" / name).read_text()
        for token, value in values.items():
            content = content.replace("{{" + token + "}}", value)
        if re.search(r"\{\{[A-Z_]+\}\}", content):
            raise ValueError(f"安装文档仍有未解析的构建字段：{name}")
        (directory / name).write_text(content)


def distribution_runtime_contract(
    runtime_profile: str, runtime_base: str | None
) -> dict:
    contract = {
        "runtime_profile": runtime_profile,
        "runtime_base": runtime_base,
        "identity_binding": "runtime-getuid-geteuid-getgid-getegid",
        "environment_root_fallback": False,
        "device_validation": "待验证真实平台来源、目录保护、HiShell 和内核隔离能力",
    }
    if runtime_profile == "platform":
        contract.update(
            {
                "platform_files_candidate": "/data/storage/el2/base/files",
                "directory_source_order": [
                    "native-application-context",
                    "validated-platform-namespace",
                ],
                "data_layout": {
                    "root": "codex",
                    "state": "codex/state",
                    "runtime_aliases": "codex/r/a",
                    "runtime_sockets": "codex/r/s",
                    "tmp": "codex/tmp",
                    "logs": "codex/logs",
                    "host": "codex/host",
                },
                "directory_validation": "持有 FD；核验实际身份、0700、祖先保护、禁止链接与防替换；不修改已有父目录",
                "initialization": "CLI 在配置、aliases、socket 和线程之前初始化；不依赖 GUI",
            }
        )
    elif runtime_profile == "hdc-debug":
        contract["deployment_scope"] = "仅 HDC shell UID 2000 诊断，不是通用 HiShell 包"
    else:
        contract["deployment_scope"] = "显式固定候选诊断，不是通用 HiShell 包"
    return contract


def build_hnp_alias(
    output: Path, *, sdk: Path, java: str, env: dict[str, str]
) -> tuple[Path, dict]:
    source = REPO_ROOT / "scripts/harmony_hnp_alias.c"
    unsigned = output / "hnp-alias-build/hnp-alias"
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
            str(source),
            "-o",
            str(unsigned),
        ],
        env,
    )
    signed = output / "已签名/hnp-alias"
    signature = strip_and_sign(unsigned, signed, sdk=sdk, java=java)
    return signed, {
        "源码": "scripts/harmony_hnp_alias.c",
        "源码摘要": digest(source),
        "SHA-256": digest(signed),
        "入口": list(HNP_ALIAS_NAMES),
        "签名": signature,
    }


def assemble(
    directory: Path,
    *,
    cli: Path,
    helpers: Path,
    runtime_probe: Path,
    version: str,
    source_commit: str,
    runtime_profile: str = "platform",
    hnp_alias: Path | None = None,
) -> None:
    if not re.fullmatch(r"[0-9a-f]{40}", source_commit):
        raise ValueError("安装文档要求完整的小写 Git 提交 SHA")
    if hnp_alias is not None and runtime_profile != "platform":
        raise ValueError("HNP 工具入口只用于通用 platform 包")
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
    for script in ("install.sh", "enable-terminal.sh", "diagnose.sh"):
        shutil.copyfile(REPO_ROOT / "scripts/harmony" / script, directory / script)
        (directory / script).chmod(0o755)
    if hnp_alias is not None:
        if not inspect_ohos_elf(hnp_alias)["签名节存在"]:
            raise RuntimeError("HNP 工具入口缺少签名节")
        for name in HNP_ALIAS_NAMES:
            shutil.copyfile(hnp_alias, directory / "codex-path" / name)
            (directory / "codex-path" / name).chmod(0o755)
    identity = (
        f"版本：`{version}`。源码提交：`{source_commit}`。"
        f"目标：ARM64 OHOS；运行目录策略：`{runtime_profile}`。\n\n"
    )
    if runtime_profile == "hdc-debug":
        emulator_docs = REPO_ROOT / "docs/鸿蒙电脑模拟器"
        tools = directory / "emulator-tools"
        tools.mkdir()
        for name in (
            "start-codex.sh",
            "run-harmony-codex.sh",
            "run-harmony-codex.command",
            "run-harmony-codex.exp",
        ):
            shutil.copyfile(emulator_docs / name, tools / name)
            (tools / name).chmod(0o755)
        shutil.copyfile(
            emulator_docs / "模拟器使用与日志速用.md",
            directory / "模拟器使用与日志速用.md",
        )
        (directory / "安装说明.md").write_text(
            "# 鸿蒙模拟器诊断包\n\n" + identity
            + "本包只适用于 HDC shell UID 2000，不是商业 PC HiShell 通用包。"
            "运行根 `/data/local/tmp/cdx` 必须由该身份创建为0700；仍核验平台祖先、锁和控制套接字。"
            "按《模拟器使用与日志速用.md》部署完整包和单独提供的 config.toml，"
            "并写入 active-package 安装记录。包内不包含 URL、Key 或用户配置。\n\n"
            "安装器使用 install.sh；PATH 环境入口为 env.sh，终端启用为 enable-terminal.sh，"
            "诊断为 diagnose.sh。安装器不执行 CLI、不改 HOME 或用户配置。"
            "目录检查不代表内核隔离可用，不自动切换完全访问。设备与认证验证未执行。\n"
        )
        return
    if runtime_profile != "platform":
        (directory / "安装说明.md").write_text(
            "# 鸿蒙固定候选诊断包\n\n" + identity
            + "本包使用显式 strict 诊断策略，不能作为通用 HiShell 验收结果。"
            "完整目录布局必须保留。安装器为 install.sh，环境入口为 env.sh，"
            "终端启用为 enable-terminal.sh，诊断为 diagnose.sh。"
            "实际固定候选见构建与签名记录.json；设备与认证验证未执行。\n"
        )
        return
    render_install_tutorials(
        directory,
        version=version,
        source_commit=source_commit,
        runtime_profile=runtime_profile,
    )
    hnp_note = (
        "\n本包包含经过签名并将摘要编入 CLI 的 HNP 工具入口，可供官方 private HNP 随 HAP 安装。"
        "不绑定应用 UID；实际安装、目录保护和不同身份运行由设备报告验收。"
        "GUI/HAP 不是 HiShell 原生 CLI 初始化的前置条件。\n"
        if hnp_alias is not None
        else ""
    )
    (directory / "安装说明.md").write_text(
        "# 鸿蒙 PC 原生候选包\n\n" + identity
        + "按《新版安装与运行说明.md》校验、解压、执行 install.sh 并加载 env.sh；"
        "认证二选一见《接口密钥安装速用.md》或《账号登录安装速用.md》。"
        "首次实际 CLI 启动负责验证平台目录并初始化 codex/state、r、tmp、logs。"
        "不使用固定应用 UID，也不要求 GUI 先启动。\n\n"
        "安装器只写程序前缀和 PATH 环境入口，不运行 CLI、不改 HOME 或认证配置。"
        "目录与内核能力不足会保留失败，不自动回退到完全访问。"
        "包内独立探针是只读观察，不批准数据根，不代表 CLI 初始化已通过。\n\n"
        "所有 ELF 使用 SDK 自签名；保留完整布局与文件校验清单。"
        "程序重新签名或修改后必须重新制作整个包。"
        "不使用普通 Linux/npm 更新包覆盖。真机、登录和真实模型调用均待验收。\n"
        + hnp_note
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
        (REPO_ROOT / "scripts/harmony/version-source.json").read_text()
    )
    if version_source["工作区版本"] != upstream_version:
        raise ValueError("工作区版本与版本来源记录不一致；请先核对上游 tag 和源码")
    identity = source_identity()
    version = package_version(upstream_version, identity["提交"])
    runtime_profile = args.runtime_profile
    runtime_base = (
        runtime_base_contract(args.runtime_base)
        if args.runtime_base is not None
        else None
    )
    env = runtime_build_environment(env, runtime_base, runtime_profile)
    env = hnp_alias_build_environment(env, runtime_profile, None)
    runtime_contract = distribution_runtime_contract(runtime_profile, runtime_base)
    helpers = args.helpers_dir.resolve()
    helper_record = validate_helpers(helpers, sdk, args.java)
    hnp_alias = None
    hnp_alias_record = None
    if args.with_hnp_alias:
        hnp_alias, hnp_alias_record = build_hnp_alias(
            output, sdk=sdk, java=args.java, env=env
        )
        env = hnp_alias_build_environment(
            env, runtime_profile, hnp_alias_record["SHA-256"]
        )
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
            "HNP工具入口": hnp_alias_record,
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
            *(["--runtime-base", runtime_base] if runtime_base is not None else []),
            "--runtime-profile",
            runtime_profile,
            *(
                ["--hnp-alias-sha256", hnp_alias_record["SHA-256"]]
                if hnp_alias_record
                else []
            ),
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
        runtime_base=runtime_base,
        runtime_profile=runtime_profile,
    )
    verify_source_unchanged(identity)
    directory = output / "鸿蒙Codex"
    assemble(
        directory,
        cli=signed_cli,
        helpers=helpers,
        runtime_probe=signed_probe,
        version=version,
        source_commit=identity["提交"],
        runtime_profile=runtime_profile,
        hnp_alias=hnp_alias,
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
    if hnp_alias_record:
        for name in HNP_ALIAS_NAMES:
            relative = f"codex-path/{name}"
            files[relative] = inspect_ohos_elf(directory / relative)
            if files[relative]["SHA-256"] != hnp_alias_record["SHA-256"]:
                raise RuntimeError("HNP 工具入口摘要与编入 CLI 的摘要不一致")
    record = {
        "版本": version,
        "版本来源": version_source,
        "目标": TARGET,
        "源码": identity,
        "Rust": TOOLCHAIN,
        "SDK": json.loads((sdk / "oh-uni-package.json").read_text()),
        "CLI签名": cli_record,
        "目录探针": probe_record,
        "HNP工具入口": hnp_alias_record,
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
            "运行目录策略": runtime_profile,
            "受保护运行根": runtime_base,
            "身份绑定": runtime_contract["identity_binding"],
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
        help="仅 strict / hdc-debug 诊断构建使用；platform 不接受固定根",
    )
    parser.add_argument(
        "--runtime-profile",
        choices=RUNTIME_PROFILES,
        default="platform",
        help="默认 platform；strict / hdc-debug 仅用于显式固定候选诊断",
    )
    parser.add_argument(
        "--with-hnp-alias", action="store_true", help="为 platform 包加入已签名的 HNP 工具入口"
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
    if args.action == "helpers" and args.runtime_base is not None:
        parser.error("helpers 不使用 --runtime-base")
    if args.action == "helpers" and args.runtime_profile != "platform":
        parser.error("helpers 不使用 --runtime-profile")
    if args.with_hnp_alias and (
        args.action != "package" or args.runtime_profile != "platform"
    ):
        parser.error("--with-hnp-alias 只适用于 package --runtime-profile platform")
    try:
        runtime_build_environment(
            {}, args.runtime_base, args.runtime_profile
        )
    except (ValueError, argparse.ArgumentTypeError) as error:
        parser.error(str(error))
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
