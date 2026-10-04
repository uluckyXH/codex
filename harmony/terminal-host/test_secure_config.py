import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent
NATIVE = ROOT / "entry/src/main/cpp"


class PrivateConfigTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        supplied = os.environ.get("CODEX_CONFIG_IMPORT_TEST_BINARY")
        if not supplied:
            raise unittest.SkipTest("Compile separately, then set CODEX_CONFIG_IMPORT_TEST_BINARY; this suite never compiles")
        cls.binary = Path(supplied).resolve(strict=True)

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.files = Path(self.temp.name)
        (self.files / "state").mkdir(mode=0o700)
        (self.files / "host").mkdir(mode=0o700)
        self.inbox = self.files / "host/handoff-private"
        self.inbox.mkdir(mode=0o700)
        self.source = self.inbox / "incoming-config.toml"

    def run_import(self, expected):
        result = subprocess.run([str(self.binary), str(self.files)], capture_output=True, text=True)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        self.assertNotIn("FAKE_TEST_KEY", result.stdout + result.stderr)

    def write_config(self, content=b'model="gpt-5.6-terra"\nkey="FAKE_TEST_KEY"\n', mode=0o660):
        self.source.write_bytes(content)
        self.source.chmod(mode)
        return content

    def test_absent_creates_only_private_directories(self):
        self.run_import(0)
        self.assertEqual((self.files / "state").stat().st_mode & 0o777, 0o700)

    def test_missing_shared_cli_state_rejected_without_creation(self):
        (self.files / "state").rmdir()
        self.run_import(3)
        self.assertFalse((self.files / "state").exists())

    def test_transfer_mode_is_saved_private_and_source_removed(self):
        content = self.write_config()
        self.run_import(2)
        final = self.files / "state/config.toml"
        self.assertEqual(final.read_bytes(), content)
        self.assertEqual(final.stat().st_mode & 0o777, 0o600)
        self.assertFalse(self.source.exists())
        self.run_import(1)

    def test_update_is_atomic_and_preserves_exact_bytes(self):
        self.write_config()
        self.run_import(2)
        content = self.write_config(b'model="gpt-5.6-terra"\n# updated\n', 0o600)
        self.run_import(2)
        self.assertEqual((self.files / "state/config.toml").read_bytes(), content)
        self.assertEqual(list((self.files / "state").glob(".config-import-*")), [])

    def test_source_symlink_and_hardlink_rejected(self):
        outside = self.files / "outside"
        outside.write_bytes(b"FAKE_TEST_KEY")
        outside.chmod(0o600)
        self.source.symlink_to(outside)
        self.run_import(3)
        self.source.unlink()
        os.link(outside, self.source)
        self.run_import(3)
        self.assertFalse((self.files / "state/config.toml").exists())

    def test_shared_directory_rejected(self):
        self.write_config()
        self.inbox.chmod(0o770)
        self.run_import(3)
        self.assertTrue(self.source.exists())

    def test_world_readable_empty_and_oversize_input_rejected(self):
        for content, mode in ((b"test", 0o644), (b"", 0o600), (b"x" * 65537, 0o600)):
            with self.subTest(mode=mode, length=len(content)):
                self.write_config(content, mode)
                self.run_import(3)
                self.assertFalse((self.files / "state/config.toml").exists())

    def test_unsafe_existing_config_rejected(self):
        self.run_import(0)
        final = self.files / "state/config.toml"
        final.write_bytes(b"FAKE_TEST_KEY")
        final.chmod(0o644)
        self.run_import(3)


if __name__ == "__main__":
    unittest.main(verbosity=2)
