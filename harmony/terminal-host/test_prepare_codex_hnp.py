"""Integrity regressions use fake runtime bytes; no device or credentials involved."""

import json
from pathlib import Path
import shutil
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import prepare_codex_hnp as staging


class HnpInputIntegrityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.source = self.root / "distribution"
        self.source.mkdir()
        self.output = self.root / "staged"
        self.revision = "a" * 40
        self.version = "0.160.0-dev.harmony.g" + self.revision[:12]
        self.record = {
            "版本": self.version,
            "目标": "aarch64-unknown-linux-ohos",
            "源码": {"提交": self.revision},
            "受保护运行根契约": {
                "编译时策略": "hnp-debug",
                "绑定应用UID": 20020059,
                "编译时固定候选": "/data/storage/el2/base/files/r",
            },
            "文件": {},
        }
        for name in staging.RUNTIME_FILES:
            path = self.source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            # Deliberately not an ELF: this test only checks provenance, never execution.
            path.write_bytes(b"non-executable runtime fixture: " + name.encode())
            self.record["文件"][name] = {"SHA-256": staging.digest(path)}
        self.metadata = {
            "layoutVersion": 1,
            "version": self.version,
            "target": "aarch64-unknown-linux-ohos",
            "variant": "codex",
            "entrypoint": "bin/codex",
            "resourcesDir": "codex-resources",
            "pathDir": "codex-path",
        }
        self.write_json(staging.BUILD_RECORD, self.record)
        self.write_json(staging.PACKAGE_METADATA, self.metadata)
        (self.source / "安装说明.md").write_text("虚拟输入，不是设备验证。\n")
        licenses = self.source / "许可原文"
        licenses.mkdir()
        (licenses / "项目许可.txt").write_text("Fixture license\n")
        self.write_checksums()

    def write_json(self, name, value):
        (self.source / name).write_text(json.dumps(value, ensure_ascii=False) + "\n")

    def write_checksums(self):
        lines = []
        for path in sorted(self.source.rglob("*")):
            if path.is_file() and path.name != staging.CHECKSUM_FILE:
                lines.append(staging.digest(path) + "  " + path.relative_to(self.source).as_posix() + "\n")
        (self.source / staging.CHECKSUM_FILE).write_text("".join(lines))

    def invoke(self):
        with patch.object(sys, "argv", ["prepare_codex_hnp.py", "--distribution", str(self.source),
                                       "--output", str(self.output), "--hnpcli", "/unused/hnpcli"]):
            staging.main()

    def rejected_before_pack(self, message):
        with patch.object(staging.subprocess, "run") as pack:
            with self.assertRaisesRegex(ValueError, message):
                self.invoke()
            pack.assert_not_called()
        self.assertFalse(self.output.exists())

    def test_valid_input_preserves_runtime_and_source_identity(self):
        def pack(command, **_):
            path = Path(command[command.index("-o") + 1]) / "codexharmony.hnp"
            path.write_bytes(b"fake SDK output, not installable")
            return SimpleNamespace(returncode=0, stdout="test pack only\n")

        with patch.object(staging.subprocess, "run", side_effect=pack) as sdk:
            self.invoke()
            sdk.assert_called_once()
        manifest = json.loads((self.output / "native-package/manifest.json").read_text())
        self.assertEqual(manifest["source"]["提交"], self.revision)
        for name in staging.RUNTIME_FILES:
            self.assertEqual((self.output / "native-package" / name).read_bytes(), (self.source / name).read_bytes())
            self.assertEqual(manifest["files"][name]["sha256"], self.record["文件"][name]["SHA-256"])
        header = (self.output / "native_package.h").read_text()
        self.assertIn("/data/app/codexharmony.org/codexharmony_0.160.0.g" + self.revision[:12], header)

    def test_tampering_each_runtime_is_rejected_by_original_checksums(self):
        for name in staging.RUNTIME_FILES:
            with self.subTest(runtime=name):
                path = self.source / name
                original = path.read_bytes()
                path.write_bytes(original + b" changed")
                self.rejected_before_pack("Distribution checksum mismatch")
                path.write_bytes(original)

    def test_rehashed_runtime_still_requires_original_build_digest(self):
        for name in staging.RUNTIME_FILES:
            with self.subTest(runtime=name):
                path = self.source / name
                original = path.read_bytes()
                path.write_bytes(original + b" changed")
                self.write_checksums()
                self.rejected_before_pack("original signed build record")
                path.write_bytes(original)
                self.write_checksums()

    def test_build_record_and_package_metadata_are_covered_by_checksums(self):
        for name in (staging.BUILD_RECORD, staging.PACKAGE_METADATA):
            with self.subTest(name=name):
                path = self.source / name
                original = path.read_bytes()
                path.write_bytes(original + b" ")
                self.rejected_before_pack("Distribution checksum mismatch")
                path.write_bytes(original)

    def test_stale_metadata_or_record_rejected_even_after_rehashing(self):
        for name, data, key in ((staging.BUILD_RECORD, self.record, "版本"),
                                (staging.PACKAGE_METADATA, self.metadata, "version")):
            with self.subTest(name=name):
                original = data[key]
                data[key] = "0.159.0-dev.harmony.gbbbbbbbbbbbb"
                self.write_json(name, data)
                self.write_checksums()
                self.rejected_before_pack("Package metadata does not match")
                data[key] = original
                self.write_json(name, data)
                self.write_checksums()

    def test_missing_checksum_entries_and_unlisted_files_are_rejected(self):
        checksum_file = self.source / staging.CHECKSUM_FILE
        original = checksum_file.read_text()
        for missing in (staging.BUILD_RECORD, staging.PACKAGE_METADATA, "bin/codex"):
            with self.subTest(missing=missing):
                checksum_file.write_text("".join(line for line in original.splitlines(keepends=True)
                                               if not line.endswith("  " + missing + "\n")))
                self.rejected_before_pack("complete file set")
        checksum_file.write_text(original)
        (self.source / "codex-path/unrecorded").write_bytes(b"unrecorded")
        self.rejected_before_pack("complete file set")

    def test_missing_runtime_file_and_build_digest_are_rejected(self):
        path = self.source / "bin/codex"
        original = path.read_bytes()
        path.unlink()
        self.write_checksums()
        self.rejected_before_pack("Missing required runtime file")
        path.write_bytes(original)
        del self.record["文件"]["bin/codex"]
        self.write_json(staging.BUILD_RECORD, self.record)
        self.write_checksums()
        self.rejected_before_pack("original signed build record")

    def test_escaping_and_ambiguous_checksum_paths_are_rejected(self):
        checksum_file = self.source / staging.CHECKSUM_FILE
        original = checksum_file.read_text()
        for name in ("../outside", "/outside", "bin/../../outside", "./bin/codex", "bin//codex", "bin\\codex"):
            with self.subTest(name=name):
                checksum_file.write_text(original + "0" * 64 + "  " + name + "\n")
                self.rejected_before_pack("Unsafe checksum path")

    def test_duplicate_entries_are_rejected(self):
        checksum_file = self.source / staging.CHECKSUM_FILE
        original = checksum_file.read_text()
        checksum_file.write_text(original + original.splitlines(keepends=True)[0])
        self.rejected_before_pack("Duplicate or self-referential")

    def test_symbolic_links_are_rejected_even_when_bytes_match(self):
        path = self.source / "bin/codex"
        outside = self.root / "outside"
        path.rename(outside)
        path.symlink_to(outside)
        self.rejected_before_pack("link or special file")
        path.unlink()
        outside.rename(path)
        directory = self.source / "codex-path"
        directory.rename(self.root / "outside-directory")
        directory.symlink_to(self.root / "outside-directory", target_is_directory=True)
        self.rejected_before_pack("link or special file")

    def test_copy_is_checked_against_previously_validated_digest(self):
        original_copy = shutil.copy2

        def corrupt_copy(source, destination, **kwargs):
            result = original_copy(source, destination, **kwargs)
            if Path(source).relative_to(self.source).as_posix() == "bin/codex":
                Path(destination).write_bytes(b"changed after validation")
            return result

        with patch.object(staging.shutil, "copy2", side_effect=corrupt_copy), \
                patch.object(staging.subprocess, "run") as pack:
            with self.assertRaisesRegex(RuntimeError, "Copy differs from verified distribution"):
                self.invoke()
            pack.assert_not_called()


if __name__ == "__main__":
    unittest.main(verbosity=2)
