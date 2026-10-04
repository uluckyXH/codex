#!/usr/bin/env python3
"""Stage a complete signed platform runtime using the SDK's normal HNP format.

Chinese delivery documents remain in the outer distribution. This script never
reads an installed user's config, builds an ELF, or changes signing permissions.
"""
import argparse
import hashlib
import json
import re
from pathlib import Path, PurePosixPath
import shutil
import stat
import subprocess

CHECKSUM_FILE = "文件校验清单.sha256"
BUILD_RECORD = "构建与签名记录.json"
PACKAGE_METADATA = "codex-package.json"
RUNTIME_FILES = (
    "bin/codex",
    "codex-path/rg",
    "codex-resources/bwrap",
    "codex-resources/harmony-runtime-probe",
    "codex-path/apply_patch",
    "codex-path/applypatch",
    "codex-path/codex-linux-sandbox",
    "codex-path/codex-execve-wrapper",
)
ALIAS_FILES = RUNTIME_FILES[4:]


def validate_runtime_contract(record, checksums):
    contract = record.get("受保护运行根契约", {})
    expected = {
        "runtime_profile": "platform",
        "runtime_base": None,
        "identity_binding": "runtime-getuid-geteuid-getgid-getegid",
        "platform_files_candidate": "/data/storage/el2/base/files",
        "directory_source_order": ["native-application-context", "validated-platform-namespace"],
        "data_layout": {
            "root": "codex", "state": "codex/state", "runtime_aliases": "codex/r/a",
            "runtime_sockets": "codex/r/s", "tmp": "codex/tmp", "logs": "codex/logs", "host": "codex/host",
        },
    }
    if not isinstance(contract, dict) or any(key not in contract or contract[key] != value for key, value in expected.items()):
        raise ValueError("HNP requires the shared platform directory/identity contract")
    for key in ("绑定应用UID", "application_uid", "runtime_uid", "编译时固定候选", "编译时策略"):
        if key in contract:
            raise ValueError("Installation-specific identity/root bindings must not enter a platform HNP")
    alias = record.get("HNP工具入口")
    if not isinstance(alias, dict) or not re.fullmatch(r"[0-9a-f]{64}", alias.get("SHA-256", "")) or any(
        checksums[name] != alias["SHA-256"] for name in ALIAS_FILES
    ):
        raise ValueError("HNP requires the original signed alias entry and matching installed aliases")
    return contract


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def checksum_path(name):
    path = PurePosixPath(name)
    if (
        not name
        or path.is_absolute()
        or any(part in {"", ".", ".."} for part in name.split("/"))
        or "\\" in name
        or any(ord(char) < 32 or ord(char) == 127 for char in name)
    ):
        raise ValueError("Unsafe checksum path: " + repr(name))
    return path


def verified_json(source, name, checksums):
    contents = (source / name).read_bytes()
    if hashlib.sha256(contents).hexdigest() != checksums[name]:
        raise ValueError("Distribution changed during validation: " + name)
    value = json.loads(contents)
    if not isinstance(value, dict):
        raise ValueError("Expected a JSON object: " + name)
    return value


def validate_distribution(source):
    """Check the existing release evidence before deriving any new manifest.

    The checksum list is an integrity record from the original distribution, not
    a cryptographic proof against replacement of that entire distribution.
    """
    actual_files = set()
    for path in source.rglob("*"):
        relative = path.relative_to(source).as_posix()
        checksum_path(relative)
        mode = path.lstat().st_mode
        if not (stat.S_ISDIR(mode) or stat.S_ISREG(mode)):
            raise ValueError("Distribution contains a link or special file: " + relative)
        if path.name in {"config.toml", "auth.json", ".env"}:
            raise ValueError("User configuration must never be packaged")
        if stat.S_ISREG(mode) and relative != CHECKSUM_FILE:
            actual_files.add(relative)
    checksum_file = source / CHECKSUM_FILE
    if not checksum_file.is_file() or checksum_file.is_symlink():
        raise ValueError("Missing regular distribution checksum list")
    checksums = {}
    for line in checksum_file.read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        if match is None:
            raise ValueError("Invalid distribution checksum entry")
        expected, name = match.groups()
        checksum_path(name)
        if name == CHECKSUM_FILE or name in checksums:
            raise ValueError("Duplicate or self-referential checksum entry: " + name)
        checksums[name] = expected
    if actual_files != checksums.keys():
        raise ValueError("Distribution checksum entries do not match its complete file set")
    required = {*RUNTIME_FILES, BUILD_RECORD, PACKAGE_METADATA}
    if not required <= checksums.keys():
        raise ValueError("Missing required runtime file or original build evidence")
    for name, expected in checksums.items():
        if digest(source / name) != expected:
            raise ValueError("Distribution checksum mismatch: " + name)
    record = verified_json(source, BUILD_RECORD, checksums)
    recorded_files = record.get("文件", {})
    for name in RUNTIME_FILES:
        if recorded_files.get(name, {}).get("SHA-256") != checksums[name]:
            raise ValueError("Runtime differs from its original signed build record: " + name)
    metadata = verified_json(source, PACKAGE_METADATA, checksums)
    expected_metadata = {
        "layoutVersion": 1,
        "version": record.get("版本"),
        "target": "aarch64-unknown-linux-ohos",
        "variant": "codex",
        "entrypoint": "bin/codex",
        "resourcesDir": "codex-resources",
        "pathDir": "codex-path",
    }
    if record.get("目标") != expected_metadata["target"] or any(
        metadata.get(key) != value for key, value in expected_metadata.items()
    ):
        raise ValueError("Package metadata does not match the original build record/layout")
    return record, checksums


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--distribution", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--hnpcli", type=Path, required=True, help="Official SDK hnpcli executable")
    args = parser.parse_args()
    source = args.distribution.resolve(strict=True)
    output = args.output.resolve()
    if output.exists():
        raise SystemExit("Output must be a new directory; existing evidence is never overwritten")
    record, checksums = validate_distribution(source)
    contract = validate_runtime_contract(record, checksums)
    revision = record["源码"]["提交"]
    if len(revision) != 40 or any(c not in "0123456789abcdef" for c in revision):
        raise SystemExit("Expected a complete lowercase source revision")
    version = re.match(r"^\d+\.\d+\.\d+", record["版本"])
    if version is None:
        raise SystemExit("Distribution version must start with an official semantic version")
    runtime_identity = hashlib.sha256(json.dumps(
        {name: checksums[name] for name in RUNTIME_FILES}, sort_keys=True, separators=(",", ":")
    ).encode()).hexdigest()
    # Dirty builds can share a Git HEAD. Include the original signed runtime
    # identity so an installed package cannot retain stale code at that HEAD.
    hnp_version = version.group(0) + ".g" + revision[:12] + ".r" + runtime_identity[:12]
    inputs = []
    for name in ["bin", "codex-path", "codex-resources"]:
        for path in sorted((source / name).rglob("*")):
            relative = path.relative_to(source)
            if not str(relative).isascii() or path.is_symlink():
                raise SystemExit("HNP runtime requires ASCII paths and regular files: " + str(relative))
            if path.name in {"config.toml", "auth.json", ".env"}:
                raise SystemExit("User configuration must never be packaged")
            if path.is_file():
                inputs.append((path, relative))
    inputs.append((source / "codex-package.json", Path("codex-package.json")))
    package = output / "native-package"
    package.mkdir(parents=True)
    files = {}
    for original, relative in inputs:
        destination = package / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(original, destination)
        expected = checksums[relative.as_posix()]
        if digest(destination) != expected:
            raise RuntimeError("Copy differs from verified distribution: " + str(relative))
        files[str(relative)] = {"sha256": expected, "bytes": destination.stat().st_size, "source": str(relative)}
    licenses = source / "许可原文"
    if licenses.is_dir():
        for index, original in enumerate(sorted(p for p in licenses.rglob("*") if p.is_file()), 1):
            if original.is_symlink():
                raise SystemExit("License symlink is not supported")
            relative = Path("licenses") / f"license-{index:03}.txt"
            destination = package / relative
            destination.parent.mkdir(exist_ok=True)
            shutil.copy2(original, destination)
            original_name = original.relative_to(source).as_posix()
            expected = checksums[original_name]
            if digest(destination) != expected:
                raise RuntimeError("License copy differs from verified distribution: " + original_name)
            files[str(relative)] = {"sha256": expected, "bytes": destination.stat().st_size, "source": original_name}
    (package / "manifest.json").write_text(json.dumps({"distribution_version": record["版本"], "source": record["源码"], "runtime_contract": contract, "files": files}, ensure_ascii=False, indent=2) + "\n")
    (package / "hnp.json").write_text(json.dumps({"type": "hnp-config", "name": "codexharmony", "version": hnp_version, "install": {"links": [{"source": "/bin/codex", "target": "codex"}]}}, indent=2) + "\n")
    installed = "/data/app/codexharmony.org/codexharmony_" + hnp_version
    (output / "native_package.h").write_text("#pragma once\n#define CODEX_HNP_PACKAGE_PATH " + json.dumps(installed) + "\n")
    hnp_output = output / "hnp/arm64-v8a"
    hnp_output.mkdir(parents=True)
    result = subprocess.run([str(args.hnpcli), "pack", "-i", str(package), "-o", str(hnp_output)], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    (output / "hnp-pack.txt").write_text(result.stdout)
    if result.returncode:
        raise SystemExit(result.stdout)
    hnp = hnp_output / "codexharmony.hnp"
    (output / "manifest.json").write_text(json.dumps({"hnp": str(hnp), "hnp_version": hnp_version, "runtime_identity": runtime_identity, "installed_package": installed, "sha256": digest(hnp), "bytes": hnp.stat().st_size, "runtime_files": files}, ensure_ascii=False, indent=2) + "\n")
    print(hnp)
    print("sha256=" + digest(hnp))


if __name__ == "__main__":
    main()
