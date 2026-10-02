"""无认证、无目标执行的打包、路径和损坏输入回归。"""

import io
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from build_harmony import REPO_ROOT, TARGET
from build_harmony_distribution import (
    package_version,
    verify_source_unchanged,
    write_checksums,
)
from build_harmony_helpers import source_archive, unpack
from harmony_elf import inspect_ohos_elf
from sign_harmony import run_tool

os.environ.setdefault("CODEX_REPO_ROOT", str(REPO_ROOT))
from codex_package.layout import build_package_dir, validate_package_dir
from codex_package.ripgrep import resolve_rg_bin
from codex_package.targets import PACKAGE_VARIANTS, PackageInputs, TARGET_SPECS
from codex_package.zsh import resolve_zsh_bin


def elf_fixture(path: Path, *, ohos: bool = True, rpath: bool = False) -> Path:
    """Minimal parseable fixture, never executable or evidence of a real build."""
    data = bytearray(176)
    sections = [(0,) * 10]
    names = b"\0.shstrtab\0.note.ohos.ident\0.dynstr\0.dynamic\0.codesign\0"

    def add(name: bytes, kind: int, value: bytes) -> None:
        offset = len(data)
        data.extend(value)
        sections.append(
            (names.index(name + b"\0"), kind, 0, 0, offset, len(value), 0, 0, 1, 0)
        )

    interpreter = b"/lib/ld-musl-aarch64.so.1\0"
    interp_offset = len(data)
    data.extend(interpreter)
    add(b".shstrtab", 3, names)
    note = struct.pack("<III", 5, 4, 1) + (b"OHOS\0" if ohos else b"GNUX\0") + b"\0" * 7
    add(b".note.ohos.ident", 7, note)
    add(b".dynstr", 3, b"\0libc.so\0/host/lib\0")
    dynamic = struct.pack("<qQ", 1, 1)
    if rpath:
        dynamic += struct.pack("<qQ", 29, 9)
    add(b".dynamic", 6, dynamic + struct.pack("<qQ", 0, 0))
    add(b".codesign", 1, b"fixture signature is not verified")
    section_offset = len(data)
    for section in sections:
        data.extend(struct.pack("<IIQQQQIIQQ", *section))
    data[:16] = b"\x7fELF\x02\x01\x01" + b"\0" * 9
    struct.pack_into(
        "<HHIQQQIHHHHHH",
        data,
        16,
        3,
        183,
        1,
        0,
        64,
        section_offset,
        0,
        64,
        56,
        2,
        64,
        len(sections),
        1,
    )
    struct.pack_into(
        "<IIQQQQQQ",
        data,
        64,
        3,
        4,
        interp_offset,
        0,
        0,
        len(interpreter),
        len(interpreter),
        1,
    )
    struct.pack_into("<IIQQQQQQ", data, 120, 1, 5, 0, 0, 0, len(data), len(data), 4096)
    path.write_bytes(data)
    path.chmod(0o755)
    return path


class DistributionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def test_upstream_versions_remain_semver_when_suffixing_fork_identity(self):
        from codex_package.cli import parse_package_version

        commit = "a" * 40
        for upstream, expected in (
            ("0.0.0", "0.0.0-harmony.aaaaaaaaaaaa"),
            ("1.2.3-beta.4", "1.2.3-beta.4.harmony.aaaaaaaaaaaa"),
            ("1.2.3+build", "1.2.3-harmony.aaaaaaaaaaaa+build"),
        ):
            result = package_version(upstream, commit)
            self.assertEqual(result, expected)
            self.assertEqual(parse_package_version(result), result)

    def test_modified_content_cannot_hide_behind_unchanged_git_status(self):
        before = {
            "提交": "a" * 40,
            "工作区状态": "M same.py",
            "工作区补丁摘要": "before",
            "时间": "1",
        }
        with patch(
            "build_harmony_distribution.source_identity",
            return_value={**before, "时间": "2"},
        ):
            verify_source_unchanged(before)
        with patch(
            "build_harmony_distribution.source_identity",
            return_value={**before, "工作区补丁摘要": "after"},
        ):
            with self.assertRaisesRegex(RuntimeError, "源码发生变化"):
                verify_source_unchanged(before)

    def test_elf_rejects_wrong_architecture_missing_note_and_host_rpath(self):
        valid = elf_fixture(self.root / "valid")
        self.assertEqual(inspect_ohos_elf(valid)["动态依赖"], ["libc.so"])
        for name, kwargs in (("linux", {"ohos": False}), ("rpath", {"rpath": True})):
            with self.subTest(name=name), self.assertRaises(RuntimeError):
                inspect_ohos_elf(elf_fixture(self.root / name, **kwargs))
        binary = bytearray(valid.read_bytes())
        struct.pack_into("<H", binary, 18, 62)
        valid.write_bytes(binary)
        with self.assertRaisesRegex(RuntimeError, "ARM64"):
            inspect_ohos_elf(valid)

    def test_elf_rejects_truncation_and_invalid_section_bounds(self):
        valid = elf_fixture(self.root / "valid")
        data = valid.read_bytes()
        for changed in (data[:30], data[:-12]):
            valid.write_bytes(changed)
            with self.assertRaises(RuntimeError):
                inspect_ohos_elf(valid)

    def test_ohos_package_preserves_bytes_without_code_mode_or_zsh(self):
        binary = elf_fixture(self.root / "input")
        package = self.root / "package"
        package.mkdir()
        spec = TARGET_SPECS[TARGET]
        variant = PACKAGE_VARIANTS["codex"]
        inputs = PackageInputs(binary, None, binary, None, binary, None, None)
        build_package_dir(package, "0.0.0-harmony.test", variant, spec, inputs)
        validate_package_dir(package, variant, spec, include_zsh=False)
        self.assertFalse((package / "bin/codex-code-mode-host").exists())
        for name in ("bin/codex", "codex-path/rg", "codex-resources/bwrap"):
            self.assertEqual((package / name).read_bytes(), binary.read_bytes())
        (package / "codex-resources/bwrap").unlink()
        with self.assertRaisesRegex(RuntimeError, "bwrap"):
            validate_package_dir(package, variant, spec, include_zsh=False)

    def test_ohos_rejects_linux_inputs_before_creating_layout(self):
        binary = elf_fixture(self.root / "linux", ohos=False)
        package = self.root / "package"
        package.mkdir()
        with self.assertRaises(RuntimeError):
            build_package_dir(
                package,
                "0.0.0",
                PACKAGE_VARIANTS["codex"],
                TARGET_SPECS[TARGET],
                PackageInputs(binary, None, binary, None, binary, None, None),
            )
        self.assertEqual(list(package.iterdir()), [])

    def test_missing_ohos_resources_do_not_download_linux_artifacts(self):
        spec = TARGET_SPECS[TARGET]
        with patch("codex_package.ripgrep.fetch_rg") as fetch:
            with self.assertRaisesRegex(RuntimeError, "OHOS"):
                resolve_rg_bin(spec, None)
            fetch.assert_not_called()
        with patch("codex_package.zsh.fetch_dotslash_executable") as fetch:
            self.assertIsNone(resolve_zsh_bin(spec))
            with self.assertRaisesRegex(RuntimeError, "OHOS"):
                resolve_zsh_bin(spec, zsh_bin=self.root / "zsh")
            fetch.assert_not_called()

    def test_source_cache_rejects_corruption_without_network(self):
        (self.root / "libcap-2.78.tar.xz").write_bytes(b"bad archive")
        with patch("build_harmony_helpers.urlopen") as download:
            with self.assertRaisesRegex(RuntimeError, "SHA-256"):
                source_archive("libcap", self.root, offline=True)
            download.assert_not_called()

    def test_source_extraction_rejects_parent_path(self):
        archive = self.root / "source.tar"
        with tarfile.open(archive, "w") as stream:
            member = tarfile.TarInfo("libcap-2.78/../../escape")
            member.size = 1
            stream.addfile(member, io.BytesIO(b"x"))
        with self.assertRaisesRegex(RuntimeError, "非法路径"):
            unpack("libcap", archive, self.root / "source")
        self.assertFalse((self.root / "escape").exists())

    def test_signing_requires_exit_status_and_success_marker(self):
        for status, stdout in ((1, "sign success"), (0, "input invalid")):
            with patch(
                "sign_harmony.subprocess.run",
                return_value=subprocess.CompletedProcess([], status, stdout, ""),
            ):
                with self.assertRaisesRegex(RuntimeError, "执行失败"):
                    run_tool("java", self.root / "tool.jar", ["sign"], "sign success")

    def test_bwrap_directory_compat_handles_long_and_deleted_cwd(self):
        source = self.root / "directory.c"
        source.write_text(r"""
#include "ohos_compat.h"
#include <assert.h>
#include <fcntl.h>
#include <string.h>
#include <sys/stat.h>
int main(void) {
    char expected[4096];
    assert(getcwd(expected, sizeof(expected)) != NULL);
    char *actual = get_current_dir_name();
    assert(actual != NULL && strcmp(actual, expected) == 0);
    free(actual);
    int parent = open(".", O_RDONLY);
    assert(parent >= 0 && mkdir("removed", 0700) == 0);
    assert(chdir("removed") == 0 && rmdir("../removed") == 0);
    errno = 0;
    assert(get_current_dir_name() == NULL && errno == ENOENT);
    assert(fchdir(parent) == 0);
    close(parent);
    return 0;
}
""")
        program = self.root / "directory-test"
        subprocess.run(
            [
                "cc",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-I",
                str(REPO_ROOT / "codex-rs/bwrap"),
                str(source),
                "-o",
                str(program),
            ],
            check=True,
        )
        subprocess.run([str(program)], cwd=self.root, check=True)
        long = self.root / ("a" * 100) / ("b" * 100) / ("c" * 100)
        long.mkdir(parents=True)
        subprocess.run([str(program)], cwd=long, check=True)


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.package = self.root / "候选 包"
        self.package.mkdir()
        shutil.copyfile(REPO_ROOT / "scripts/harmony/安装.sh", self.package / "安装.sh")
        for name in ("bin/codex", "codex-path/rg", "codex-resources/bwrap"):
            path = self.package / name
            path.parent.mkdir()
            path.write_text("#!/bin/sh\nexit 97\n")
            path.chmod(0o755)
        write_checksums(self.package)

    def install(self, prefix: Path) -> subprocess.CompletedProcess:
        return subprocess.run(
            ["/bin/sh", str(self.package / "安装.sh"), "--prefix", str(prefix)],
            text=True,
            capture_output=True,
        )

    def test_install_preserves_bytes_permissions_and_quotes_path(self):
        prefix = self.root / "中文 空格'$(touch 不应执行)`touch 不应执行2`"
        result = self.install(prefix)
        self.assertEqual(result.returncode, 0, result.stderr)
        path = subprocess.check_output(
            [
                "/bin/sh",
                "-c",
                '. "$1"; command -v codex',
                "sh",
                str(prefix / "环境.sh"),
            ],
            text=True,
            cwd=self.root,
        ).strip()
        self.assertEqual(path, str(prefix / "bin/codex"))
        self.assertEqual(
            (prefix / "bin/codex").read_bytes(),
            (self.package / "bin/codex").read_bytes(),
        )
        self.assertTrue(os.access(prefix / "codex-resources/bwrap", os.X_OK))
        self.assertFalse((self.root / "不应执行").exists())
        self.assertFalse((self.root / "不应执行2").exists())

    def test_existing_prefix_is_preserved(self):
        prefix = self.root / "old"
        prefix.mkdir()
        (prefix / "保留").write_text("original")
        result = self.install(prefix)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((prefix / "保留").read_text(), "original")
        self.assertEqual(len(list(prefix.iterdir())), 1)

    def test_corrupted_package_never_creates_install_directory(self):
        (self.package / "codex-resources/bwrap").write_text("changed")
        prefix = self.root / "new"
        result = self.install(prefix)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(prefix.exists())

    def test_missing_resource_or_newline_prefix_is_rejected(self):
        for name in ("bad\npath", "bad:path"):
            prefix = self.root / name
            self.assertNotEqual(self.install(prefix).returncode, 0)
            self.assertFalse(prefix.exists())
        (self.package / "codex-path/rg").unlink()
        prefix = self.root / "missing"
        self.assertNotEqual(self.install(prefix).returncode, 0)
        self.assertFalse(prefix.exists())


if __name__ == "__main__":
    unittest.main()
