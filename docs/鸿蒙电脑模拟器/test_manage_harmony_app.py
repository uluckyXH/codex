#!/usr/bin/env python3
"""Exercise the HDC helper with fake transport and non-secret configuration."""

from contextlib import redirect_stdout, redirect_stderr
import importlib.util
import io
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "harmony_app_manager", Path(__file__).with_name("manage-harmony-app.py")
)
manager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(manager)


class AppManagerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.hdc = self.root / "hdc"
        self.hdc.touch()
        self.config = self.root / "config.toml"
        self.config.write_text(
            'model = "gpt-5.6-terra"\nmodel_provider = "proxy"\n'
            '[model_providers.proxy]\nwire_api = "responses"\n'
            'base_url = "https://example.invalid/v1"\n'
            'experimental_bearer_token = "TEST_TOKEN_NEVER_SENT"\n'
        )
        self.config.chmod(0o600)
        self.commands = []
        self.reply = lambda command: ""

        def run(command, **_kwargs):
            self.commands.append(command)
            return subprocess.CompletedProcess(command, 0, self.reply(command), "")

        self.addCleanup(patch.stopall)
        patch.object(manager.subprocess, "run", side_effect=run).start()
        patch.object(manager.time, "sleep").start()

    def invoke(self, *arguments):
        output = io.StringIO()
        with redirect_stdout(output), redirect_stderr(output):
            manager.main(["--hdc", str(self.hdc), *arguments])
        return output.getvalue()

    def test_bundle_command_injection_rejected_before_transport(self):
        with self.assertRaises(SystemExit):
            self.invoke("--app", "com.codex.test; echo injected", "open")
        self.assertEqual(self.commands, [])

    def test_second_bundle_uses_same_logical_layout_without_uid(self):
        self.invoke("--app", "com.codex.second", "open")
        self.assertEqual(self.commands[0][-1], "aa start -a EntryAbility -b com.codex.second")
        self.assertNotIn("20020059", " ".join(self.commands[0]))

    def test_log_export_redacts_and_uses_private_new_destination(self):
        self.reply = lambda command: "TEST_TOKEN_NEVER_SENT https://example.invalid/v1 example.invalid\n"
        target = self.root / "export"
        self.invoke("--app", "com.codex.second", "collect-logs", "--config", str(self.config), "--output", str(target))
        saved = target / "codex-tui-redacted.log"
        self.assertNotIn("TEST_TOKEN_NEVER_SENT", saved.read_text())
        self.assertNotIn("example.invalid", saved.read_text())
        self.assertEqual(stat.S_IMODE(saved.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o700)
        self.assertEqual(self.commands[0][-2], "com.codex.second")
        self.assertIn("/codex/logs/codex-tui.log", self.commands[0][-1])

    def test_import_works_without_previous_completion_marker(self):
        marker_reads = 0
        report = manager.DATA + "/logs/codex-import-111-1.status.txt"

        def reply(command):
            nonlocal marker_reads
            if command[-1].startswith("if [ -f "):
                marker_reads += 1
                return "" if marker_reads == 1 else report + "\n"
            if command[-1] == "cat ." + report:
                return "config_import_stage=complete result=2 errno=0\n"
            return ""

        self.reply = reply
        self.invoke("--app", "com.codex.second", "import-config", "--config", str(self.config))
        transfers = [command for command in self.commands if "send" in command]
        self.assertEqual(len(transfers), 2)
        self.assertEqual(transfers[0][-1], "." + manager.HOST + "/handoff-private/incoming-config.toml")
        self.assertEqual(transfers[1][-1], "." + manager.HOST + "/control/action.txt")
        self.assertTrue(all("TEST_TOKEN_NEVER_SENT" not in " ".join(command) for command in self.commands))

    def test_status_refuses_path_outside_expected_log_directory(self):
        self.reply = lambda command: "/data/storage/el2/base/files/codex/state/config.toml\n"
        with self.assertRaisesRegex(RuntimeError, "No completed native action"):
            self.invoke("status")
        self.assertEqual(len(self.commands), 1)

    def test_failed_native_diagnostic_is_not_reported_as_success(self):
        marker_reads = 0
        report = manager.DATA + "/logs/codex-version-112-1.status.txt"

        def reply(command):
            nonlocal marker_reads
            if command[-1].startswith("if [ -f "):
                marker_reads += 1
                return "" if marker_reads == 1 else report + "\n"
            if command[-1] == "cat ." + report:
                return "exit_code=1 signal=0\n"
            return ""

        self.reply = reply
        with self.assertRaisesRegex(RuntimeError, "did not exit successfully"):
            self.invoke("version")

    def test_stale_completion_does_not_satisfy_new_action(self):
        report = manager.DATA + "/logs/codex-version-112-1.status.txt"
        self.reply = lambda command: report + "\n" if command[-1].startswith("if [ -f ") else ""
        with self.assertRaisesRegex(RuntimeError, "No fresh native completion"):
            self.invoke("version")
        self.assertFalse(any(command[-1] == "cat ." + report for command in self.commands))


if __name__ == "__main__":
    unittest.main()
