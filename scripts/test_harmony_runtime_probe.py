"""独立原生探针宿主回归；仅使用新建固定数据，不运行 Codex 或认证代码。"""

import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest


SOURCE = Path(__file__).with_name("harmony_runtime_probe.c").resolve()
CC = shutil.which("clang") or shutil.which("cc")


def compile_c(source, output, *flags):
    command = [
        CC,
        "-std=c11",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-O1",
        *flags,
        str(source),
        "-o",
        str(output),
    ]
    if sys.platform != "darwin":
        command.append("-ldl")
    completed = subprocess.run(command, capture_output=True, text=True)
    if completed.returncode:
        raise RuntimeError(f"C compile failed: {command!r}\n{completed.stdout}{completed.stderr}")


class RuntimeProbeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not CC:
            raise RuntimeError("需要宿主 C 编译器")
        cls.build = tempfile.TemporaryDirectory(prefix="codex-probe-build-")
        cls.addClassCleanup(cls.build.cleanup)
        cls.build_root = Path(cls.build.name).resolve()
        cls.binary = cls.build_root / "probe"
        compile_c(
            SOURCE,
            cls.binary,
            '-DCODEX_HARMONY_BUILD_ID="fixture-build"',
            '-DCODEX_HARMONY_VERSION="0.160.0-dev"',
        )

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="codex-probe-data-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.env = {
            "PATH": "/nonexistent",
            "HOME": str(self.root),
            "CODEX_HOME": str(self.root / "config"),
            "TMPDIR": str(self.root),
            "LANG": "C",
        }
        (self.root / "config").mkdir()

    def run_probe(self, *args, code=0, binary=None):
        completed = subprocess.run(
            [str(binary or self.binary), *args],
            env=self.env,
            capture_output=True,
            text=True,
            timeout=20,
        )
        self.assertEqual(completed.returncode, code, completed.stdout + completed.stderr)
        report = json.loads(completed.stdout)
        self.assertLess(len(completed.stdout.encode()), 1024 * 1024)
        self.assertFalse(report["runtime_root_approved"])
        return report

    @staticmethod
    def records(report, stage):
        return [check for check in report["checks"] if check["stage"] == stage]

    def test_help_does_not_need_process_or_directory_probes(self):
        self.env["HOME"] = "/nonexistent"
        completed = subprocess.run(
            [str(self.binary), "--help"], env=self.env, capture_output=True, text=True, timeout=2
        )
        self.assertEqual(completed.returncode, 0)
        self.assertIn("Exit 0", completed.stdout)
        self.assertEqual(completed.stderr, "")

    def test_identity_uses_native_calls_with_no_external_commands(self):
        report = self.run_probe("--json", "--path", str(self.root))
        identity = self.records(report, "identity")[0]["result"]
        self.assertEqual(identity["uid"], os.getuid())
        self.assertEqual(identity["euid"], os.geteuid())
        self.assertEqual(identity["gid"], os.getgid())
        self.assertEqual(identity["egid"], os.getegid())
        self.assertEqual(set(identity["groups"]), set(os.getgroups()))
        self.assertEqual(report["build_id"], "fixture-build")
        self.assertEqual(report["version"], "0.160.0-dev")

    def test_read_only_never_opens_dotenv_or_configuration_contents(self):
        # A FIFO would block if startup tried to consume .env. No credentials exist.
        os.mkfifo(self.root / "config/.env")
        marker = self.root / "config/config.toml"
        marker.write_text("fixed fixture text: do not collect this content")
        before = sorted(str(path.relative_to(self.root)) for path in self.root.rglob("*"))
        report = self.run_probe()
        self.assertTrue(report["read_only"])
        self.assertNotIn("fixed fixture text", json.dumps(report))
        self.assertEqual(before, sorted(str(path.relative_to(self.root)) for path in self.root.rglob("*")))

    def test_explicit_paths_replace_defaults_and_report_missing_errno(self):
        path = self.root / "missing"
        report = self.run_probe("--path", str(path))
        checks = self.records(report, "path")
        self.assertEqual(len(checks), 1)
        self.assertEqual(checks[0]["candidate"], str(path))
        self.assertEqual(checks[0]["result"]["ancestors"][-1]["errno"], 2)

    def test_unicode_quotes_and_control_characters_are_valid_json(self):
        path = self.root / '中文 "quoted"\nfolder'
        path.mkdir()
        report = self.run_probe("--path", str(path))
        self.assertEqual(self.records(report, "path")[0]["result"]["path"], str(path))

    def test_symlink_is_observed_without_approving_its_chain(self):
        actual = self.root / "actual"
        actual.mkdir()
        link = self.root / "link"
        link.symlink_to(actual, target_is_directory=True)
        report = self.run_probe("--path", str(link))
        observation = self.records(report, "path")[0]["result"]
        self.assertFalse(observation["posix_ancestor_checks"])
        self.assertTrue(observation["ancestors"][-1]["symlink"])
        self.assertEqual(observation["canonical"]["path"], str(actual))
        self.assertEqual(observation["canonical_ancestors"][-1]["ino"], actual.stat().st_ino)

    def test_create_roundtrip_and_cleanup_leave_existing_content_unchanged(self):
        sentinel = self.root / "keep"
        sentinel.write_bytes(b"fixed existing content")
        before = self.root.stat()
        report = self.run_probe("--path", str(self.root), "--create-test", str(self.root))
        creation = self.records(report, "create")[0]["result"]
        self.assertTrue(creation["created"])
        self.assertTrue(creation["file_roundtrip"])
        self.assertTrue(creation["socket_bind"])
        self.assertEqual(creation["created_metadata"]["mode"], "0700")
        self.assertEqual(creation["cleanup"], "removed")
        self.assertFalse(creation["runtime_root_approved"])
        self.assertEqual(sentinel.read_bytes(), b"fixed existing content")
        self.assertEqual(self.root.stat().st_mode, before.st_mode)
        self.assertFalse(list(self.root.glob(".codex-probe-*")))

    def test_group_writable_setgid_parent_is_rejected_without_modification(self):
        parent = self.root / "shared"
        parent.mkdir()
        parent.chmod(0o2771)
        before = parent.stat()
        report = self.run_probe("--path", str(parent), "--create-test", str(parent), code=1)
        creation = self.records(report, "create")[0]["result"]
        self.assertEqual(creation["status"], "parent_rejected")
        self.assertFalse(creation["created"])
        self.assertEqual(parent.stat().st_mode, before.st_mode)
        self.assertEqual(list(parent.iterdir()), [])

    def test_symlink_parent_is_rejected_without_creating_at_destination(self):
        actual = self.root / "actual"
        actual.mkdir()
        link = self.root / "link"
        link.symlink_to(actual, target_is_directory=True)
        report = self.run_probe("--path", str(link), "--create-test", str(link), code=1)
        self.assertEqual(self.records(report, "create")[0]["result"]["status"], "parent_rejected")
        self.assertEqual(list(actual.iterdir()), [])

    def test_root_parent_is_rejected_including_redundant_slashes(self):
        for path in ("/", "//", "///"):
            with self.subTest(path=path):
                report = self.run_probe("--path", str(self.root), "--create-test", path, code=1)
                creation = self.records(report, "create")[0]["result"]
                self.assertEqual(creation["status"], "parent_rejected")
                self.assertFalse(creation["created"])

    def test_argument_bounds_and_dot_components_fail_before_probing(self):
        arguments = [
            ["--path", "relative"],
            ["--path", "/tmp/../other"],
            ["--path", "/tmp/./other"],
            ["--path", "/" + "x" * 1024],
            ["--path", "/tmp"] * 13,
            ["--create-test", "/tmp", "--create-test", "/tmp"],
            ["--unknown"],
        ]
        for args in arguments:
            with self.subTest(args=args):
                completed = subprocess.run(
                    [str(self.binary), *args], env=self.env, capture_output=True, timeout=2
                )
                self.assertEqual(completed.returncode, 2)
                self.assertEqual(completed.stdout, b"")

    def test_native_platform_unavailable_is_an_observation(self):
        report = self.run_probe("--path", str(self.root))
        self.assertEqual(len(self.records(report, "native")), 3)
        for check in self.records(report, "native"):
            self.assertEqual(check["result"]["status"], "unsupported_platform")

    def context_binary(self, name, body):
        library_source = self.root / f"{name}.c"
        library = self.root / f"{name}.dylib"
        library_source.write_text(
            "#include <stdint.h>\n#include <signal.h>\n#include <unistd.h>\n"
            "#include <stdio.h>\n#include <string.h>\n"
            "int OH_AbilityRuntime_ApplicationContextGetFilesDir(char *p, int32_t n, int32_t *l) {"
            "(void)p; (void)n; (void)l;" + body + "}\n"
        )
        compile_c(library_source, library, "-shared", "-fPIC")
        binary = self.root / name
        compile_c(
            SOURCE,
            binary,
            f'-DPROBE_TEST_CONTEXT_LIBRARY="{library}"',
            "-DPROBE_ITEM_TIMEOUT_MS=1500",
        )
        return binary

    def test_native_no_context_and_missing_symbols_are_distinct(self):
        binary = self.context_binary("no-context", "return 16000011;")
        report = self.run_probe("--path", str(self.root), binary=binary)
        checks = self.records(report, "native")
        self.assertEqual(checks[0]["result"]["status"], "context_not_exist")
        self.assertEqual(checks[0]["result"]["return_code"], 16000011)
        self.assertEqual(checks[1]["result"]["status"], "symbol_unavailable")

    def test_native_signal_retains_wait_status_and_other_probes_continue(self):
        binary = self.context_binary("signal-context", "raise(SIGSYS); return 1;")
        report = self.run_probe("--path", str(self.root), binary=binary, code=1)
        check = self.records(report, "native")[0]
        self.assertEqual(check["signal"], signal.SIGSYS)
        self.assertIsNone(check["exit_code"])
        self.assertTrue(os.WIFSIGNALED(check["raw_wait_status"]))
        self.assertIn("native-context-call", check["stderr"])
        self.assertEqual(self.records(report, "path")[0]["status"], "collected")

    def test_native_timeout_does_not_block_remaining_checks(self):
        binary = self.context_binary("slow-context", "sleep(5); return 1;")
        started = time.monotonic()
        report = self.run_probe("--path", str(self.root), binary=binary, code=1)
        check = self.records(report, "native")[0]
        self.assertTrue(check["timed_out"])
        self.assertIn("native-context-call", check["stderr"])
        self.assertLess(time.monotonic() - started, 5)
        self.assertEqual(self.records(report, "path")[0]["status"], "collected")

    def test_native_invalid_path_is_reported_without_reading_it(self):
        binary = self.context_binary("invalid-context", "*l=1; p[0]='x'; p[1]=0; return 0;")
        report = self.run_probe("--path", str(self.root), binary=binary)
        self.assertEqual(self.records(report, "native")[0]["result"]["status"], "invalid_api_path")

    def test_native_success_observes_path_without_approving_root(self):
        body = 'snprintf(p, (size_t)n, "%s", ' + json.dumps(str(self.root)) + ');'
        binary = self.context_binary("valid-context", body + "*l=(int32_t)strlen(p); return 0;")
        report = self.run_probe("--path", str(self.root), binary=binary)
        native = self.records(report, "native")[0]["result"]
        self.assertEqual(native["status"], "ok")
        self.assertEqual(native["observation"]["path"], str(self.root))
        self.assertFalse(native["observation"]["runtime_root_approved"])

    def test_native_library_unavailable_remains_diagnostic(self):
        binary = self.root / "missing-library"
        compile_c(SOURCE, binary, f'-DPROBE_TEST_CONTEXT_LIBRARY="{self.root}/missing.dylib"')
        report = self.run_probe("--path", str(self.root), binary=binary)
        for check in self.records(report, "native"):
            self.assertEqual(check["result"]["status"], "library_unavailable")

    def test_oversized_worker_output_is_bounded_and_does_not_break_json(self):
        binary = self.context_binary(
            "noisy-context",
            'char chunk[1024]; memset(chunk,\'x\',sizeof(chunk));'
            'for(int i=0;i<64;i++) if(write(3,chunk,sizeof(chunk))<0) break; return 1;',
        )
        report = self.run_probe("--path", str(self.root), binary=binary, code=1)
        check = self.records(report, "native")[0]
        self.assertTrue(check["output_truncated"])
        self.assertLessEqual(len(check["partial_output"]), 16384)
        self.assertEqual(self.records(report, "path")[0]["status"], "collected")

    def test_ancestor_and_cleanup_identity_rules_use_real_objects(self):
        harness = self.root / "rules.c"
        binary = self.root / "rules"
        harness.write_text(
            '#define main probe_program_main\n#include "' + str(SOURCE) + '"\n#undef main\n'
            "#include <assert.h>\n"
            "int main(int argc, char **argv) {\n"
            " assert(argc == 2); struct stat st = {0}; st.st_uid=geteuid(); st.st_mode=S_IFDIR|0700;\n"
            " assert(safe_ancestor(&st)); st.st_mode=S_IFDIR|02771; assert(!safe_ancestor(&st));\n"
            " st.st_uid=geteuid()+1; st.st_mode=S_IFDIR|0700; assert(!safe_ancestor(&st));\n"
            " st.st_uid=0; st.st_mode=S_IFDIR|01777; assert(safe_ancestor(&st));\n"
            " int parent=open(argv[1], O_RDONLY|O_DIRECTORY); assert(parent>=0);\n"
            " int first=openat(parent, \"first\", O_CREAT|O_EXCL|O_RDWR,0600); assert(first>=0);\n"
            " assert(fstat(first,&st)==0); assert(same_entry(parent,\"first\",&st));\n"
            " assert(renameat(parent,\"first\",parent,\"original\")==0);\n"
            " int second=openat(parent,\"first\",O_CREAT|O_EXCL|O_RDWR,0600); assert(second>=0);\n"
            " assert(!same_entry(parent,\"first\",&st));\n"
            " close(second); close(first); close(parent); return 0; }\n"
        )
        compile_c(harness, binary)
        subprocess.run([str(binary), str(self.root)], check=True, timeout=2)
        self.assertTrue((self.root / "first").exists())
        self.assertTrue((self.root / "original").exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
