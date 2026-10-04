"""Pure build-parameter tests; never call Hvigor, a compiler, SDK or a device."""
import argparse
import json
from pathlib import Path
import tempfile
import unittest
import build_terminal_host as build


class BundleParameterTests(unittest.TestCase):
    def test_hvigor_checks_resolved_output_path_before_staging(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.assertEqual(build.output_directory(root / "host-build"), (root / "host-build").resolve())
            target = root / "中文缓存"
            target.mkdir()
            alias = root / "current"
            alias.symlink_to(target, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "ASCII path"):
                build.output_directory(alias / "host-build")

    def test_two_bundle_identities_only_change_copied_app_metadata(self):
        source = (build.ROOT / "AppScope/app.json5").read_bytes()
        with tempfile.TemporaryDirectory() as temporary:
            projects = []
            for index, name in enumerate(("com.codex.emulatorhnp", "com.codex.emulatorhnp.second")):
                project = Path(temporary) / str(index)
                (project / "AppScope").mkdir(parents=True)
                (project / "AppScope/app.json5").write_bytes(source)
                (project / "signed-elf-fixture").write_bytes(b"same original fixture; not executable")
                digest = build.configure_bundle(project, name)
                self.assertEqual(digest, build.sha256(project / "AppScope/app.json5"))
                declaration = json.loads((project / "AppScope/app.json5").read_text())
                self.assertEqual(declaration["app"].pop("bundleName"), name)
                projects.append((declaration, (project / "signed-elf-fixture").read_bytes()))
            self.assertEqual(projects[0], projects[1])
        self.assertEqual((build.ROOT / "AppScope/app.json5").read_bytes(), source)

    def test_invalid_or_shell_like_bundle_names_rejected(self):
        for value in ("short", "com..codex", "com.codex;id", "com.codex/other", "com.编码", "com.codex\n", "x." + "a" * 126):
            with self.subTest(value=value), self.assertRaises(argparse.ArgumentTypeError):
                build.bundle_name(value)


if __name__ == "__main__":
    unittest.main(verbosity=2)
