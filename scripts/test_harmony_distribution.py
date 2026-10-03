"""无认证、无目标执行的打包、路径和损坏输入回归。"""

import io
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from build_harmony import REPO_ROOT, TARGET
from build_harmony_distribution import (
    assemble,
    build_package,
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
            ("0.160.0-dev", "0.160.0-dev.harmony.gaaaaaaaaaaaa"),
            ("0.160.0", "0.160.0-harmony.gaaaaaaaaaaaa"),
            ("1.2.3-beta.4", "1.2.3-beta.4.harmony.gaaaaaaaaaaaa"),
            ("1.2.3+build", "1.2.3-harmony.gaaaaaaaaaaaa+build"),
        ):
            result = package_version(upstream, commit)
            self.assertEqual(result, expected)
            self.assertEqual(parse_package_version(result), result)

    def test_numeric_and_invalid_commit_identities(self):
        self.assertEqual(
            package_version("0.160.0-dev", "012345678901" + "0" * 28),
            "0.160.0-dev.harmony.g012345678901",
        )
        for commit in ("", "short", "G" * 40, "a" * 39 + "\n"):
            with (
                self.subTest(commit=commit),
                self.assertRaisesRegex(ValueError, "提交 SHA"),
            ):
                package_version("0.160.0-dev", commit)

    def test_version_resolution_does_not_require_a_preset_repository_env(self):
        env = dict(os.environ)
        env.pop("CODEX_REPO_ROOT", None)
        result = subprocess.run(
            [
                sys.executable,
                "-c",
                "from build_harmony_distribution import package_version; "
                "print(package_version('0.160.0-dev', 'a' * 40))",
            ],
            cwd=REPO_ROOT / "scripts",
            env=env,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "0.160.0-dev.harmony.gaaaaaaaaaaaa")

    def test_placeholder_versions_cannot_be_released(self):
        for version in ("0.0.0", "0.0.0-dev", "0.0.0+build"):
            with (
                self.subTest(version=version),
                self.assertRaisesRegex(ValueError, "占位版本"),
            ):
                package_version(version, "a" * 40)

    def test_version_provenance_mismatch_stops_before_building_or_signing(self):
        (self.root / "codex-rs").mkdir()
        (self.root / "codex-rs/Cargo.toml").write_text(
            '[workspace.package]\nversion = "0.160.0-dev"\n'
        )
        provenance = self.root / "scripts/harmony/版本来源.json"
        provenance.parent.mkdir(parents=True)
        provenance.write_text(json.dumps({"工作区版本": "0.159.3"}))
        with (
            patch("build_harmony_distribution.REPO_ROOT", self.root),
            patch("build_harmony_distribution.validate_helpers") as helpers,
            patch("build_harmony_distribution.run") as run,
        ):
            with self.assertRaisesRegex(ValueError, "版本来源记录不一致"):
                build_package(SimpleNamespace(), self.root, self.root, {})
            helpers.assert_not_called()
            run.assert_not_called()

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

    def test_distribution_includes_signed_probe_and_copyable_tutorials(self):
        binary = elf_fixture(self.root / "input")
        helpers = self.root / "helpers"
        (helpers / "已签名").mkdir(parents=True)
        (helpers / "许可原文").mkdir()
        for name in ("rg", "bwrap"):
            shutil.copyfile(binary, helpers / "已签名" / name)
        package = self.root / "full-package"
        assemble(
            package,
            cli=binary,
            helpers=helpers,
            runtime_probe=binary,
            version="0.160.0-dev.harmony.gaaaaaaaaaaaa",
        )
        probe = package / "codex-resources/harmony-runtime-probe"
        self.assertEqual(probe.read_bytes(), binary.read_bytes())
        self.assertTrue(os.access(probe, os.X_OK))
        for name in (
            "接口密钥安装速用.md",
            "账号登录安装速用.md",
            "升级与专项日志速用.md",
            "七版修复包安装与复测.md",
        ):
            self.assertEqual(
                (package / name).read_bytes(),
                (REPO_ROOT / "docs/鸿蒙电脑原生适配" / name).read_bytes(),
            )
        write_checksums(package)
        manifest = (package / "文件校验清单.sha256").read_text()
        self.assertIn("  codex-resources/harmony-runtime-probe\n", manifest)
        self.assertIn("  升级与专项日志速用.md\n", manifest)

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
        for name in ("安装.sh", "启用终端.sh", "诊断.sh"):
            shutil.copyfile(REPO_ROOT / "scripts/harmony" / name, self.package / name)
        for name in (
            "bin/codex",
            "codex-path/rg",
            "codex-resources/bwrap",
            "codex-resources/harmony-runtime-probe",
        ):
            path = self.package / name
            path.parent.mkdir(exist_ok=True)
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

    def test_environment_can_be_sourced_repeatedly_without_path_growth(self):
        prefix = self.root / "installed"
        self.assertEqual(self.install(prefix).returncode, 0)
        script = '. "$1"; first=$PATH; . "$1"; test "$first" = "$PATH"'
        subprocess.run(["sh", "-c", script, "sh", str(prefix / "环境.sh")], check=True)

    def activate(self, prefix: Path, rc_file: Path, *options: str):
        return subprocess.run(
            ["sh", str(prefix / "启用终端.sh"), "--rc-file", str(rc_file), *options],
            text=True,
            capture_output=True,
        )

    def test_activation_preserves_zsh_content_and_is_idempotent_and_reversible(self):
        prefix = self.root / "中文 '$() 安装"
        self.assertEqual(self.install(prefix).returncode, 0)
        rc_file = self.root / "测试终端配置"
        original = "# 原有配置\narray=(one two)\nsetopt INTERACTIVE_COMMENTS\n"
        rc_file.write_text(original)
        first = self.activate(prefix, rc_file)
        self.assertEqual(first.returncode, 0, first.stderr)
        activated = rc_file.read_text()
        self.assertTrue(activated.startswith(original))
        backups = list(self.root.glob("测试终端配置.鸿蒙Codex-*.bak"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_text(), original)
        second = self.activate(prefix, rc_file)
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(rc_file.read_text(), activated)
        self.assertEqual(len(list(self.root.glob("测试终端配置.鸿蒙Codex-*.bak"))), 1)
        self.assertEqual(self.activate(prefix, rc_file, "--remove").returncode, 0)
        self.assertEqual(rc_file.read_text(), original)

    def test_activation_updates_to_new_install_and_does_not_duplicate_block(self):
        old = self.root / "old-version"
        new = self.root / "new-version"
        self.assertEqual(self.install(old).returncode, 0)
        self.assertEqual(self.install(new).returncode, 0)
        rc_file = self.root / "测试终端配置"
        self.assertEqual(self.activate(old, rc_file).returncode, 0)
        self.assertEqual(self.activate(new, rc_file).returncode, 0)
        content = rc_file.read_text()
        self.assertNotIn(str(old), content)
        self.assertIn(str(new), content)
        self.assertEqual(content.count("# >>> 鸿蒙 Codex 环境 >>>"), 1)
        path = subprocess.check_output(
            ["zsh", "-f", "-c", '. "$1"; command -v codex', "zsh", str(rc_file)],
            text=True,
        ).strip()
        self.assertEqual(path, str(new / "bin/codex"))

    def test_activation_rejects_symlink_or_broken_markers_without_changing_original(
        self,
    ):
        prefix = self.root / "installed"
        self.assertEqual(self.install(prefix).returncode, 0)
        original = self.root / "原文件"
        original.write_text("# keep\n")
        rc_file = self.root / "测试终端配置"
        rc_file.symlink_to(original)
        self.assertNotEqual(self.activate(prefix, rc_file).returncode, 0)
        self.assertEqual(original.read_text(), "# keep\n")
        rc_file.unlink()
        for content in (
            "# >>> 鸿蒙 Codex 环境 >>>\n# unfinished\n",
            "# <<< 鸿蒙 Codex 环境 <<<\n",
            "# >>> 鸿蒙 Codex 环境 >>>\n# <<< 鸿蒙 Codex 环境 <<<\n" * 2,
            "unfinished=(\n",
        ):
            rc_file.write_text(content)
            self.assertNotEqual(self.activate(prefix, rc_file).returncode, 0)
            self.assertEqual(rc_file.read_text(), content)

    def test_diagnostic_retains_failures_without_dumping_environment(self):
        prefix = self.root / "installed"
        self.assertEqual(self.install(prefix).returncode, 0)
        output = self.root / "诊断输出"
        env = dict(os.environ, HARMONY_TEST_PRIVATE="private-sentinel-do-not-export")
        result = subprocess.run(
            ["sh", str(prefix / "诊断.sh"), "--output-dir", str(output)],
            env=env,
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(result.returncode, 0)
        summary = (output / "检查摘要.txt").read_text()
        self.assertIn("版本：退出码 97", summary)
        self.assertIn("包内沙箱版本：退出码 97", summary)
        self.assertIn("目录与身份：退出码 97", summary)
        texts = "".join(p.read_text() for p in output.glob("*.txt"))
        self.assertNotIn("private-sentinel-do-not-export", texts)
        self.assertFalse((output / "受限终端.txt").exists())
        again = subprocess.run(
            ["sh", str(prefix / "诊断.sh"), "--output-dir", str(output)],
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(again.returncode, 0)
        self.assertEqual((output / "检查摘要.txt").read_text(), summary)

    def test_path_only_probe_does_not_initialize_codex_or_need_a_report_directory(self):
        prefix = self.root / "installed"
        self.assertEqual(self.install(prefix).returncode, 0)
        (prefix / "bin/codex").write_text("#!/bin/sh\nexit 98\n")
        (prefix / "codex-resources/harmony-runtime-probe").write_text(
            '#!/bin/sh\n[ "$#" -eq 1 ] && [ "$1" = --json ] || exit 99\n'
            'printf \'{"fixture":"no-target-code-executed","uid":20020101}\\n\'\n'
        )
        env = dict(os.environ)
        env.pop("HOME", None)
        result = subprocess.run(
            ["sh", str(prefix / "诊断.sh"), "--paths-only"],
            env=env,
            text=True,
            capture_output=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["uid"], 20020101)
        self.assertEqual(result.stderr, "")
        conflict = self.root / "must-not-create"
        result = subprocess.run(
            [
                "sh",
                str(prefix / "诊断.sh"),
                "--paths-only",
                "--output-dir",
                str(conflict),
            ],
            env=env,
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(conflict.exists())

    def test_new_package_cannot_install_without_standalone_probe(self):
        (self.package / "codex-resources/harmony-runtime-probe").unlink()
        prefix = self.root / "missing-probe"
        result = self.install(prefix)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(prefix.exists())

    def test_diagnostic_sandbox_probe_preserves_policy_and_exit_159(self):
        prefix = self.root / "installed"
        self.assertEqual(self.install(prefix).returncode, 0)
        # Executable fixture only; no real Codex, model or operating-system sandbox runs.
        (prefix / "bin/codex").write_text("""#!/bin/sh
case "$1" in
    --version|doctor) exit 0 ;;
    -c)
        [ "$2" = 'sandbox_mode="read-only"' ] || exit 91
        [ "$3" = sandbox ] && [ "$4" = -- ] || exit 92
        case "$5" in /usr/bin/sh|/bin/sh) ;; *) exit 93 ;; esac
        [ "$6" = -c ] && [ "$7" = pwd ] || exit 93
        [ "$CODEX_HARMONY_PROCESS_DIAGNOSTICS" = 1 ] || exit 94
        printf 'simulated exit 159\\n'
        exit 159 ;;
    *) exit 95 ;;
esac
""")
        (prefix / "codex-resources/bwrap").write_text("#!/bin/sh\nexit 0\n")
        write_checksums(prefix)
        output = self.root / "沙箱诊断输出"
        result = subprocess.run(
            ["sh", str(prefix / "诊断.sh"), "--sandbox", "--output-dir", str(output)],
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(result.returncode, 0)
        summary = (output / "检查摘要.txt").read_text()
        self.assertIn("普通终端：退出码 0", summary)
        self.assertIn("受限终端：退出码 159", summary)
        self.assertIn("simulated exit 159", (output / "受限终端.txt").read_text())

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

    def test_unlisted_files_and_environment_symlink_are_not_copied(self):
        outside = self.root / "普通外部文件"
        outside.write_text("must remain unchanged")
        (self.package / "环境.sh").symlink_to(outside)
        (self.package / "未列入清单.txt").write_text("not part of the package")
        prefix = self.root / "safe-install"
        result = self.install(prefix)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(outside.read_text(), "must remain unchanged")
        self.assertFalse((prefix / "环境.sh").is_symlink())
        self.assertTrue((prefix / "环境.sh").is_file())
        self.assertFalse((prefix / "未列入清单.txt").exists())

    def test_manifest_files_and_parent_directories_cannot_be_symlinks(self):
        for index, name in enumerate(("bin/codex", "bin", "文件校验清单.sha256")):
            with self.subTest(name=name):
                path = self.package / name
                saved = self.root / f"saved-{index}"
                path.rename(saved)
                path.symlink_to(saved, target_is_directory=saved.is_dir())
                try:
                    prefix = self.root / f"rejected-{index}"
                    result = self.install(prefix)
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    self.assertFalse(prefix.exists())
                finally:
                    path.unlink()
                    saved.rename(path)

    def test_manifest_rejects_escape_and_reserved_names_before_install(self):
        manifest = self.package / "文件校验清单.sha256"
        original = manifest.read_text()
        for index, name in enumerate(
            (
                "环境.sh",
                "文件校验清单.sha256",
                "../外部文件",
                "/tmp/文件",
                "bin/../bin/codex",
                "bin//codex",
            )
        ):
            with self.subTest(name=name):
                manifest.write_text(original + "0" * 64 + "  " + name + "\n")
                prefix = self.root / f"invalid-{index}"
                result = self.install(prefix)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertFalse(prefix.exists())


if __name__ == "__main__":
    unittest.main()
