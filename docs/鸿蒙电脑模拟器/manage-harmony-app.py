#!/usr/bin/env python3
"""Mac HDC helper for the dedicated HarmonyOS Codex debug application."""

import argparse
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import time
import tomllib
from urllib.parse import urlsplit

APP = "com.codex.emulatorhnp"
FILES = "/data/storage/el2/base/files"
DATA = FILES + "/codex"
HOST = DATA + "/host"
COMPLETION = HOST + "/launch-completion.txt"
DEFAULT_HDC = "/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/toolchains/hdc"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hdc", type=Path, default=Path(DEFAULT_HDC))
    parser.add_argument("--target", default="127.0.0.1:5555")
    parser.add_argument("--app", default=APP, help="Installed debug bundle; no UID is required")
    parser.add_argument(
        "action",
        choices=[
            "open",
            "prepare",
            "version",
            "doctor",
            "import-config",
            "status",
            "collect-logs",
        ],
    )
    parser.add_argument("--config", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    if not args.hdc.is_file():
        parser.error("HDC not found; supply --hdc with the SDK tool path")
    if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z][A-Za-z0-9_]*)+", args.app):
        parser.error("--app must be a dotted application bundle name")
    secrets = []

    def call(*operation):
        result = subprocess.run(
            [str(args.hdc), "-t", args.target, *operation],
            capture_output=True,
            text=True,
            timeout=30,
        )
        output = result.stdout + "\n" + result.stderr
        for value in secrets:
            if value:
                output = output.replace(value, "[REDACTED]")
        if result.returncode or "[Fail]" in output:
            raise RuntimeError("HDC operation failed: " + output)
        return output

    def read_private(path, *, optional=False):
        # HDC -b addresses this bundle's logical application directory view.
        # A missing completion file is normal before the first native action.
        remote = shlex.quote("." + path)
        command = ("if [ -f " + remote + " ]; then cat " + remote + "; fi") if optional else "cat " + remote
        output = call("shell", "-b", args.app, command)
        if "No such file" in output or "Permission denied" in output:
            raise RuntimeError("Private application file is unavailable: " + path)
        return output

    if args.action == "open":
        print(call("shell", "aa start -a EntryAbility -b " + args.app).strip())
        return
    if args.action == "status":
        current = read_private(COMPLETION, optional=True)
        match = re.search(
            r"^" + re.escape(DATA) + r"/logs/codex-[a-z-]+-\d+-\d+\.status\.txt$",
            current,
            re.M,
        )
        if not match:
            raise RuntimeError("No completed native action status is available")
        print(read_private(match.group(0)).strip())
        return
    if args.action in ("import-config", "collect-logs"):
        if args.config is None:
            parser.error(args.action + " needs --config for validation/redaction")
        config = tomllib.loads(args.config.read_text())
        if args.config.stat().st_mode & 0o077:
            raise RuntimeError(
                "Config must be private: chmod 600 the config file first"
            )
        if config.get("model") != "gpt-5.6-terra":
            raise RuntimeError("The model must remain gpt-5.6-terra")
        provider = config.get("model_providers", {}).get(
            config.get("model_provider"), {}
        )
        address, token = (
            provider.get("base_url", ""),
            provider.get("experimental_bearer_token", ""),
        )
        parsed = urlsplit(address)
        if (
            config.get("model_provider") != "proxy"
            or provider.get("wire_api") != "responses"
            or not isinstance(token, str)
            or not token
        ):
            raise RuntimeError(
                "Expected the proxy Responses provider and a bearer token"
            )
        if (
            parsed.scheme != "https"
            or not parsed.hostname
            or parsed.username
            or parsed.password
            or parsed.query
            or parsed.fragment
        ):
            raise RuntimeError(
                "Expected an HTTPS base URL without embedded credentials or parameters"
            )
        secrets[:] = [token, address, parsed.hostname]
        if args.config.stat().st_size > 65536:
            raise RuntimeError("Config exceeds 64 KiB")
        if args.action == "collect-logs":
            if args.output is None:
                parser.error("collect-logs needs a new --output directory")
            output = read_private(DATA + "/logs/codex-tui.log")
            args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
            target = args.output / "codex-tui-redacted.log"
            target.write_text(output)
            target.chmod(0o600)
            print("Redacted log saved: " + str(target.resolve()))
            return
        call(
            "file",
            "send",
            "-m",
            "-b",
            args.app,
            str(args.config.resolve()),
            "." + HOST + "/handoff-private/incoming-config.toml",
        )
    native_action = "import" if args.action == "import-config" else args.action
    # This explicit diagnostic/import command restarts only the dedicated app.
    # Use `open` to show the app without interrupting an existing terminal.
    old = read_private(COMPLETION, optional=True)
    with tempfile.TemporaryDirectory(prefix="codex-hnp-action-") as temp:
        action_file = Path(temp) / "action.txt"
        action_file.write_text(native_action + "\n")
        action_file.chmod(0o600)
        call(
            "file",
            "send",
            "-m",
            "-b",
            args.app,
            str(action_file),
            "." + HOST + "/control/action.txt",
        )
    call("shell", "aa force-stop " + args.app)
    call("shell", "aa start -a EntryAbility -b " + args.app)
    pattern = (
        r"^"
        + re.escape(DATA)
        + "/logs/codex-"
        + re.escape(native_action)
        + r"-\d+-\d+\.status\.txt$"
    )
    for _ in range(25):
        current = read_private(COMPLETION, optional=True)
        match = re.search(pattern, current, re.M)
        if match and current != old:
            report = read_private(match.group(0))
            print(report.strip())
            if (
                native_action == "import"
                and "config_import_stage=complete result=2 errno=0" not in report
            ):
                raise RuntimeError("Native import did not report success")
            if (
                native_action in ("version", "doctor")
                and "exit_code=0 signal=0" not in report
            ):
                raise RuntimeError("Native diagnostic did not exit successfully")
            return
        time.sleep(2)
    raise RuntimeError("No fresh native completion; open the app and inspect status")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(str(error), file=sys.stderr)
        raise SystemExit(1)
