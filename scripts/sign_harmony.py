#!/usr/bin/env python3
"""用 SDK 官方 Java 工具自签名 OHOS ELF；不申请证书、不执行目标程序。"""

import argparse
import json
from pathlib import Path
import shutil
import subprocess

from build_harmony import native_sdk
from harmony_elf import inspect_ohos_elf


def signing_tool(sdk: Path) -> Path:
    jar = sdk.parent / "toolchains/lib/binary-sign-tool.jar"
    if not jar.is_file():
        raise RuntimeError(f"SDK 缺少官方二进制签名工具：{jar}")
    return jar


def run_tool(java: str, jar: Path, arguments: list[str], success: str) -> str:
    result = subprocess.run(
        [java, "-jar", str(jar), *arguments],
        text=True,
        capture_output=True,
        timeout=600,
    )
    log = result.stdout + result.stderr
    if result.returncode != 0 or success not in log:
        raise RuntimeError(f"签名工具执行失败（退出码 {result.returncode}）：\n{log}")
    return log


def check_signature(path: Path, *, sdk: Path, java: str) -> str:
    if not inspect_ohos_elf(path)["签名节存在"]:
        raise RuntimeError(f"程序没有 .codesign 节：{path}")
    return run_tool(
        java,
        signing_tool(sdk),
        ["display-sign", "-inFile", str(path)],
        "display-sign success",
    )


def sign_elf(source: Path, destination: Path, *, sdk: Path, java: str) -> dict:
    source = source.resolve()
    destination = destination.resolve()
    if source == destination or destination.exists():
        raise RuntimeError(f"签名输出必须是新的独立文件：{destination}")
    before = inspect_ohos_elf(source)
    if before["签名节存在"]:
        raise RuntimeError(f"输入已含签名；请使用未签名产物，避免修改旧签名：{source}")
    jar = signing_tool(sdk)
    destination.parent.mkdir(parents=True, exist_ok=True)
    try:
        sign = run_tool(
            java,
            jar,
            [
                "sign",
                "-inFile",
                str(source),
                "-outFile",
                str(destination),
                "-selfSign",
                "1",
            ],
            "sign success",
        )
        after = inspect_ohos_elf(destination)
        display = check_signature(destination, sdk=sdk, java=java)
    except Exception:
        # Only this newly-created output belongs to the failed operation.
        destination.unlink(missing_ok=True)
        raise
    destination.chmod(source.stat().st_mode & 0o777 | 0o111)
    return {
        "方式": "官方 binary-sign-tool.jar 自签名",
        "工具": str(jar),
        "输入": before,
        "输出": after,
        "签名日志": sign,
        "签名信息检查": display,
        "真机状态": "未验证；本机签名检查不代表商业鸿蒙 PC 接受加载",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--java", default=shutil.which("java"))
    parser.add_argument("--record", type=Path, required=True)
    args = parser.parse_args()
    if not args.java:
        parser.error("需要 Java 8 或以上版本，可通过 --java 指定")
    sdk = native_sdk(args.sdk)
    if args.record.exists() or args.record.resolve() in (
        args.input.resolve(),
        args.output.resolve(),
    ):
        parser.error("记录文件必须使用新的路径，且不能与输入、输出程序相同")
    record = sign_elf(args.input, args.output, sdk=sdk, java=args.java)
    args.record.parent.mkdir(parents=True, exist_ok=True)
    args.record.write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n")
    print(f"签名与静态检查完成：{args.output}；商业 PC 加载待验证")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
