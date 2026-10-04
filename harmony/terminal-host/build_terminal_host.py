#!/usr/bin/env python3
"""Build an emulator debug HAP from a signed, UID-bound Codex distribution.

All build state stays below --output. No credentials are packaged, no ELF is
rewritten, no device is changed, and no system/application signing is invented.
The emulator's unsigned-debug install mechanism is a deployment prerequisite.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parent
SOURCE_DIRS = ("AppScope", "entry/src", "hvigor")
SOURCE_FILES = ("build-profile.json5", "hvigorfile.ts", "oh-package.json5",
                "entry/build-profile.json5", "entry/hvigorfile.ts", "entry/oh-package.json5")


def sha256(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def source_files():
    files = [ROOT / name for name in SOURCE_FILES]
    for directory in SOURCE_DIRS:
        files.extend(path for path in (ROOT / directory).rglob("*") if path.is_file())
    for path in files:
        if path.is_symlink() or path.name in {"config.toml", "auth.json", ".env"} or path.suffix in {".hap", ".hnp", ".p12", ".p7b", ".key"}:
            raise ValueError("Prohibited source artifact: " + str(path.relative_to(ROOT)))
        with path.open("rb") as source:
            if source.read(4) == b"\x7fELF":
                raise ValueError("Runtime ELF must come from the verified distribution")
        yield path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--distribution", type=Path, required=True, help="Signed hnp-debug distribution directory")
    parser.add_argument("--deveco", type=Path, required=True, help="DevEco-Studio.app or its Contents directory")
    parser.add_argument("--output", type=Path, required=True, help="New output directory (on an external volume if desired)")
    args = parser.parse_args()
    deveco = args.deveco.resolve(strict=True)
    if (deveco / "Contents").is_dir():
        deveco /= "Contents"
    sdk = deveco / "sdk/default/openharmony"
    node = deveco / "tools/node/bin/node"
    java = deveco / "jbr/Contents/Home/bin/java"
    hvigor = deveco / "tools/hvigor/hvigor"
    ohpm = deveco / "tools/ohpm/bin/pm-cli.js"
    for tool in (node, java, hvigor / "bin/hvigor.js", ohpm, sdk / "toolchains/hnpcli", sdk / "toolchains/lib/app_packing_tool.jar"):
        if not tool.is_file():
            raise ValueError("Required bundled tool is missing: " + str(tool))
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    project = output / "project"
    project.mkdir()
    manifest = {}
    for path in source_files():
        relative = path.relative_to(ROOT)
        destination = project / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, destination)
        manifest[str(relative)] = sha256(path)
    # Build state and dependencies are generated only in the copied project.
    env = os.environ.copy()
    env.update(NODE_HOME=str(deveco / "tools/node"), NODE_PATH=str(project / "node_modules"),
               JAVA_HOME=str(deveco / "jbr/Contents/Home"), DEVECO_SDK_HOME=str(deveco / "sdk"),
               HVIGOR_USER_HOME=str(output / "cache/hvigor"))
    env["PATH"] = str(node.parent) + ":" + env.get("PATH", "")
    modules = project / "node_modules/@ohos"
    modules.mkdir(parents=True)
    for name in ("hvigor", "hvigor-ohos-plugin"):
        (modules / name).symlink_to(deveco / "tools/hvigor" / name, target_is_directory=True)

    def run(command, label, cwd=project):
        with (output / (label + ".log")).open("w") as log:
            result = subprocess.run([str(arg) for arg in command], cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT)
        print(label + ": exit=" + str(result.returncode), flush=True)
        if result.returncode:
            raise SystemExit("Build failed; inspect " + str(output / (label + ".log")))

    runtime = output / "runtime"
    run([sys.executable, ROOT / "prepare_codex_hnp.py", "--distribution", args.distribution.resolve(strict=True),
         "--output", runtime, "--hnpcli", sdk / "toolchains/hnpcli"], "prepare-runtime")
    shutil.copy2(runtime / "native_package.h", project / "entry/src/main/cpp/native_package.h")
    shutil.copytree(runtime / "hnp", project / "hnp")
    asset_root = project / "entry/src/main/resources/rawfile"
    for package in json.loads((asset_root / "terminal-assets.json").read_text()):
        for name, details in package["files"].items():
            if Path(name).name != name or sha256(asset_root / name) != details["sha256"]:
                raise ValueError("Pinned terminal asset integrity check failed")
    # There is only one local NAPI type dependency; remote packages are forbidden.
    for name in ("oh-package.json5", "entry/oh-package.json5"):
        declaration = json.loads((project / name).read_text())
        for group in ("dependencies", "devDependencies", "dynamicDependencies"):
            for dependency, target in declaration.get(group, {}).items():
                if dependency != "libhostprobe.so" or target != "file:./src/main/cpp/types/libhostprobe":
                    raise ValueError("Only the local NAPI declaration dependency is permitted")
    run([node, ohpm, "install", "--all", "--symlink_for_local_dep", "--cache", output / "cache/ohpm"], "local-dependencies")
    run([node, hvigor / "bin/hvigor.js", "--mode", "module", "-p", "product=default", "-p", "module=entry@default",
         "assembleHap", "--no-daemon", "--offline"], "assemble-hap")
    unpacked = output / "hap-content"
    unpacked.mkdir()
    with zipfile.ZipFile(project / "entry/build/default/outputs/default/entry-default-unsigned.hap") as archive:
        for info in archive.infolist():
            if not (unpacked / info.filename).resolve().is_relative_to(unpacked) or ((info.external_attr >> 16) & 0o170000) == 0o120000:
                raise ValueError("Invalid HAP archive member")
        archive.extractall(unpacked)
    hap = output / "codex-harmony-terminal-debug.hap"
    command = [java, "-jar", sdk / "toolchains/lib/app_packing_tool.jar", "--mode", "hap"]
    for argument, name in (("json", "module.json"), ("lib", "libs"), ("resources", "resources"), ("index", "resources.index"),
                           ("pack-info", "pack.info"), ("ets", "ets"), ("pkg-context", "pkgContextInfo.json"), ("pkg-sdk-info", "pkgSdkInfo.json")):
        if (unpacked / name).exists():
            command.extend(["--" + argument + "-path", unpacked / name])
    command.extend(["--hnp-path", project / "hnp", "--out-path", hap, "--force", "true"])
    run(command, "package-private-hnp")
    runtime_manifest = json.loads((runtime / "manifest.json").read_text())
    build_record = json.loads((args.distribution / "构建与签名记录.json").read_text())
    app = json.loads((project / "AppScope/app.json5").read_text())["app"]
    record = {"hap": hap.name, "hap_sha256": sha256(hap), "app_version": app["versionName"], "app_version_code": app["versionCode"],
              "bundle": app["bundleName"], "deployment": "unsigned emulator debug HAP; original ELF signatures preserved",
              "rust_source_sha": build_record["源码"]["提交"], "codex_version": build_record["版本"],
              "hnp": runtime_manifest, "host_source_files": manifest,
              "host_source_sha256": hashlib.sha256(json.dumps(manifest, sort_keys=True).encode()).hexdigest()}
    record["build_tools"] = {name: sha256(ROOT / name) for name in
                             ("build_terminal_host.py", "prepare_codex_hnp.py", "fetch_terminal_assets.py")}
    (output / "build-manifest.json").write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n")
    (output / (hap.name + ".sha256")).write_text(record["hap_sha256"] + "  " + hap.name + "\n")
    print(hap)


if __name__ == "__main__":
    main()
