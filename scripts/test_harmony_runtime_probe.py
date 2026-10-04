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

    def contract_binary(self, base):
        binary = self.root / "contract-probe"
        compile_c(SOURCE, binary, "-DCODEX_OHOS_RUNTIME_BASE=" + json.dumps(str(base), ensure_ascii=False))
        return binary

    def test_default_runtime_paths_use_build_contract_and_actual_euid_without_writes(self):
        base = self.root / '运行根 "quoted"'
        base.mkdir()
        binary = self.contract_binary(str(base) + "/")
        # The caller cannot override the build contract, and PATH has no id/shell.
        self.env["CODEX_OHOS_RUNTIME_BASE"] = str(self.root / "wrong-base")
        before = sorted(str(path.relative_to(self.root)) for path in self.root.rglob("*"))
        report = self.run_probe(binary=binary)
        candidates = [check["candidate"] for check in self.records(report, "path")]
        expected = [str(base / f"c{os.geteuid():08x}" / leaf) for leaf in ("a", "s")]
        self.assertEqual(candidates[:2], expected)
        self.assertEqual(report["supervisor_euid"], os.geteuid())
        self.assertEqual(report["path_selection"]["runtime_paths"], "selected")
        self.assertEqual(report["path_selection"]["selected_count"], len(candidates))
        self.assertLessEqual(len(candidates), 12)
        self.assertIn("/data/storage/el2/base/files", candidates)
        self.assertTrue(report["read_only"])
        self.assertEqual(before, sorted(str(path.relative_to(self.root)) for path in self.root.rglob("*")))

    def test_no_build_contract_does_not_guess_euid_runtime_paths(self):
        self.env["CODEX_OHOS_RUNTIME_BASE"] = str(self.root / "ignored")
        report = self.run_probe()
        self.assertEqual(report["path_selection"]["runtime_paths"], "not_configured")
        candidates = [check["candidate"] for check in self.records(report, "path")]
        self.assertFalse(any(f"/c{os.geteuid():08x}/" in candidate for candidate in candidates))
        self.assertIn("/data/storage/el2/base/files", candidates)

    def test_platform_candidates_are_uid_independent_and_never_initialize_data(self):
        binary = self.root / "platform-probe"
        compile_c(SOURCE, binary, "-DCODEX_OHOS_PLATFORM_DATA=1")
        self.env["CODEX_OHOS_RUNTIME_BASE"] = str(self.root / "ignored-root")
        before = sorted(str(path.relative_to(self.root)) for path in self.root.rglob("*"))
        report = self.run_probe(binary=binary)
        candidates = [check["candidate"] for check in self.records(report, "path")]
        self.assertEqual(candidates[:2], [
            "/data/storage/el2/base/files/codex/r/a",
            "/data/storage/el2/base/files/codex/r/s",
        ])
        self.assertEqual(report["path_selection"]["runtime_paths"], "platform_namespace_candidates")
        self.assertFalse(any(f"/c{os.geteuid():08x}/" in candidate for candidate in candidates))
        self.assertLessEqual(len(candidates), 12)
        self.assertTrue(report["read_only"])
        self.assertEqual(before, sorted(str(path.relative_to(self.root)) for path in self.root.rglob("*")))

    def test_explicit_platform_paths_replace_namespace_candidates(self):
        binary = self.root / "platform-probe"
        compile_c(SOURCE, binary, "-DCODEX_OHOS_PLATFORM_DATA=1")
        report = self.run_probe(*(["--path", str(self.root)] * 12), binary=binary)
        checks = self.records(report, "path")
        self.assertEqual(len(checks), 12)
        self.assertTrue(all(check["candidate"] == str(self.root) for check in checks))
        self.assertEqual(report["path_selection"]["runtime_paths"], "explicit_paths_override")

    def test_twelve_explicit_paths_override_configured_defaults(self):
        binary = self.contract_binary(self.root / "unused")
        report = self.run_probe(*(["--path", str(self.root)] * 12), binary=binary)
        checks = self.records(report, "path")
        self.assertEqual(len(checks), 12)
        self.assertTrue(all(check["candidate"] == str(self.root) for check in checks))
        self.assertEqual(report["path_selection"]["mode"], "explicit")
        self.assertEqual(report["path_selection"]["runtime_paths"], "explicit_paths_override")

    def test_invalid_or_oversized_contract_paths_are_reported_without_truncated_candidates(self):
        cases = [("relative", "invalid_base"), ("///", "invalid_base"),
                 ("/tmp/../other", "invalid_base"), ("/" + "x" * 1015, "path_too_long")]
        for base, status in cases:
            with self.subTest(base=base[:40], status=status):
                report = self.run_probe(binary=self.contract_binary(base))
                self.assertEqual(report["path_selection"]["runtime_paths"], status)
                candidates = [check["candidate"] for check in self.records(report, "path")]
                self.assertFalse(any(f"/c{os.geteuid():08x}/" in candidate for candidate in candidates))
                self.assertTrue(all(len(candidate.encode()) < 1024 for candidate in candidates))

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

    def test_owned_0711_ancestor_metadata_and_child_identity_are_preserved(self):
        # The owner still has read permission on Mac. This covers the unchanged
        # policy/metadata path, not execution of the OHOS-only O_PATH branch.
        ancestor = self.root / "ancestor"
        ancestor.mkdir(mode=0o711)
        child = ancestor / "child"
        child.mkdir(mode=0o700)
        before = ancestor.stat()
        report = self.run_probe("--path", str(child))
        observation = self.records(report, "path")[0]["result"]
        self.assertTrue(observation["posix_ancestor_checks"])
        metadata = next(item for item in observation["ancestors"] if item["path"] == str(ancestor))
        self.assertEqual(metadata["mode"], "0711")
        self.assertEqual(metadata["ino"], before.st_ino)
        self.assertEqual(observation["ancestors"][-1]["ino"], child.stat().st_ino)
        self.assertNotIn("errno", observation["filesystem"])
        self.assertEqual(ancestor.stat().st_mode, before.st_mode)

    def mount_fixture_report(self, lines):
        fixture = self.root / "mountinfo.fixture"
        fixture.write_text("".join(lines))
        harness = self.root / "mountinfo.c"
        binary = self.root / "mountinfo-probe"
        harness.write_text(
            '#define _GNU_SOURCE 1\n#include <stdio.h>\n#include <string.h>\n'
            'static FILE *fixture_fopen(const char *path, const char *mode) {\n'
            ' return fopen(!strcmp(path,"/proc/self/mountinfo") ? '
            + json.dumps(str(fixture)) + ' : path,mode);\n}\n'
            '#define fopen fixture_fopen\n#include "' + str(SOURCE) + '"\n'
        )
        compile_c(harness, binary)
        report = self.run_probe("--path", str(self.root), binary=binary)
        return self.records(report, "path")[0]["result"]["mountinfo"]

    def test_filtered_mountinfo_is_explicitly_not_a_complete_mount_table(self):
        # Synthetic parser input, not the device's actual mount table or ABI.
        mountinfo = self.mount_fixture_report([
            "1 0 0:1 / / rw - fixture none rw\n",
            "2 1 0:2 / /unrelated-fixture-path rw - fixture none rw\n",
        ])
        self.assertEqual(mountinfo["selection"], "covering_candidate_path")
        self.assertFalse(mountinfo["complete_mount_table"])
        self.assertFalse(mountinfo["truncated"])
        self.assertEqual([entry["mount_id"] for entry in mountinfo["entries"]], [1])

    def test_filtered_mountinfo_entry_limit_stays_bounded(self):
        mountinfo = self.mount_fixture_report([
            f"{number} 0 0:1 / / rw - fixture none rw\n" for number in range(1, 10)
        ])
        self.assertEqual(len(mountinfo["entries"]), 8)
        self.assertEqual(mountinfo["entry_limit"], 8)
        self.assertEqual(mountinfo["input_byte_limit"], 1024 * 1024)
        self.assertTrue(mountinfo["truncated"])
        self.assertFalse(mountinfo["complete_mount_table"])

    def test_changed_opened_ancestor_is_rejected_before_creating_objects(self):
        # Substitute a different real directory FD between fstatat and fstat.
        # This exercises the existing identity check with the production walk;
        # it does not simulate the kernel implementation of O_PATH on macOS.
        original = self.root / "swap-me"
        replacement = self.root / "replacement"
        original.mkdir()
        replacement.mkdir()
        harness = self.root / "changed-ancestor.c"
        binary = self.root / "changed-ancestor"
        harness.write_text(
            '#define _GNU_SOURCE 1\n#include <fcntl.h>\n#include <stdarg.h>\n'
            '#include <string.h>\n'
            'static int redirected_openat(int fd, const char *path, int flags, ...) {\n'
            '  mode_t mode=0; if(flags & O_CREAT) { va_list ap; va_start(ap,flags);'
            '    mode=(mode_t)va_arg(ap,int); va_end(ap); }\n'
            '  return openat(fd, !strcmp(path,"swap-me") ? "replacement" : path, flags, mode);\n'
            '}\n#define openat redirected_openat\n#include "' + str(SOURCE) + '"\n'
        )
        compile_c(harness, binary)
        report = self.run_probe("--path", str(original), "--create-test", str(original), binary=binary, code=1)
        self.assertFalse(self.records(report, "path")[0]["result"]["posix_ancestor_checks"])
        creation = self.records(report, "create")[0]["result"]
        self.assertEqual(creation["status"], "parent_rejected")
        self.assertFalse(creation["created"])
        self.assertEqual(list(original.iterdir()), [])
        self.assertEqual(list(replacement.iterdir()), [])

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

    def context_binary(self, name, body, *, extra_source="", all_symbols=False):
        library_source = self.root / f"{name}.c"
        library = self.root / f"{name}.dylib"
        library_source.write_text(
            "#include <stdint.h>\n#include <signal.h>\n#include <unistd.h>\n"
            "#include <stdio.h>\n#include <string.h>\n#include <stdlib.h>\n" + extra_source + "\n"
            "int OH_AbilityRuntime_ApplicationContextGetFilesDir(char *p, int32_t n, int32_t *l) {"
            "(void)p; (void)n; (void)l;" + body + "}\n" + (
                "int OH_AbilityRuntime_ApplicationContextGetCacheDir(char *p, int32_t n, int32_t *l) {"
                "return OH_AbilityRuntime_ApplicationContextGetFilesDir(p,n,l);}\n"
                "int OH_AbilityRuntime_ApplicationContextGetTempDir(char *p, int32_t n, int32_t *l) {"
                "return OH_AbilityRuntime_ApplicationContextGetFilesDir(p,n,l);}\n"
                if all_symbols else ""
            )
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
        self.assertEqual(checks[0]["result"]["write_length"], -1)
        self.assertEqual(checks[1]["result"]["status"], "symbol_unavailable")

    def assert_native_finished_without_hooks(self, report):
        for check in self.records(report, "native"):
            self.assertEqual(check["status"], "collected")
            self.assertEqual(check["exit_code"], 0)
            self.assertIsNone(check["signal"])
            self.assertTrue(os.WIFEXITED(check["raw_wait_status"]))
            self.assertEqual(check["result"]["library_lifetime"], "retained_until_worker_exit")
            phases = ("native-library-retained", "worker-result-flushed errno=0",
                      "worker-result-closed errno=0", "native-worker-_Exit code=0")
            positions = [check["stderr"].index("stage=" + phase) for phase in phases]
            self.assertEqual(positions, sorted(positions))
            self.assertNotIn("DANGEROUS_HOOK", check["stderr"])
            self.assertNotIn("native-dlclose", check["stderr"])

    def check_dangerous_library(self, name, hooks, *, unload, expected_signal, all_symbols=False):
        binary = self.context_binary(name, "return 16000011;", extra_source=hooks, all_symbols=all_symbols)
        # A separate real loader proves the hook runs and terminates a process.
        # Without this control, a harmless/inert fixture could hide regressions.
        driver = self.root / (name + "-control.c")
        control = self.root / (name + "-control")
        driver.write_text(
            "#include <dlfcn.h>\n#include <stdlib.h>\n#include <sys/resource.h>\n"
            "int main(int argc,char **argv) { if(argc!=2) return 80;"
            "struct rlimit limit={0,0}; if(setrlimit(RLIMIT_CORE,&limit)) return 84;"
            "void *h=dlopen(argv[1],RTLD_NOW|RTLD_LOCAL); if(!h) return 81;"
            + ("if(dlclose(h)) return 82; _Exit(83);" if unload else "return 0;") + "}\n"
        )
        compile_c(driver, control)
        completed = subprocess.run(
            [str(control), str(self.root / (name + ".dylib"))], env=self.env,
            capture_output=True, text=True, timeout=3,
        )
        self.assertEqual(completed.returncode, -expected_signal, completed.stderr)
        self.assertIn("DANGEROUS_HOOK", completed.stderr)
        report = self.run_probe("--path", str(self.root), binary=binary)
        self.assert_native_finished_without_hooks(report)
        self.assertEqual(self.records(report, "native")[0]["result"]["status"], "context_not_exist")
        return report

    def test_dangerous_destructor_is_not_run_for_no_context_or_missing_symbol(self):
        hooks = ('__attribute__((destructor)) static void dangerous(void) {'
                 'fputs("DANGEROUS_HOOK destructor\\n",stderr); fflush(stderr);'
                 'raise(SIGSEGV); _Exit(93);}')
        report = self.check_dangerous_library("dangerous-destructor", hooks, unload=True,
                                              expected_signal=signal.SIGSEGV)
        self.assertEqual(self.records(report, "native")[1]["result"]["status"], "symbol_unavailable")

    def test_dangerous_atexit_is_not_run_after_three_no_context_queries(self):
        hooks = ('static void dangerous(void) {'
                 'fputs("DANGEROUS_HOOK atexit\\n",stderr); fflush(stderr); raise(SIGABRT); _Exit(94);}'
                 '__attribute__((constructor)) static void setup(void) {'
                 'if(atexit(dangerous)) _Exit(95);}')
        report = self.check_dangerous_library("dangerous-atexit", hooks, unload=False,
                                              expected_signal=signal.SIGABRT, all_symbols=True)
        for check in self.records(report, "native"):
            self.assertEqual(check["result"]["status"], "context_not_exist")
            self.assertEqual(check["result"]["return_code"], 16000011)
            self.assertEqual(check["result"]["write_length"], -1)

    def test_native_result_write_and_close_failure_remains_incomplete(self):
        binary = self.context_binary("closed-result", "close(3); return 16000011;")
        report = self.run_probe("--path", str(self.root), binary=binary, code=1)
        check = self.records(report, "native")[0]
        self.assertEqual(check["status"], "incomplete")
        self.assertEqual(check["exit_code"], 1)
        self.assertEqual(os.WEXITSTATUS(check["raw_wait_status"]), 1)
        self.assertIn("partial_output", check)
        self.assertIn("stage=worker-result-flushed errno=", check["stderr"])
        self.assertNotIn("stage=worker-result-flushed errno=0", check["stderr"])
        self.assertIn("stage=native-worker-_Exit code=1", check["stderr"])
        self.assertEqual(self.records(report, "path")[0]["status"], "collected")

    def test_native_stderr_failure_does_not_report_success(self):
        binary = self.context_binary("closed-stderr", "close(2); return 16000011;")
        report = self.run_probe("--path", str(self.root), binary=binary, code=1)
        check = self.records(report, "native")[0]
        self.assertEqual(check["status"], "incomplete")
        self.assertEqual(check["exit_code"], 1)
        self.assertEqual(check["result"]["status"], "context_not_exist")

    def test_native_signal_retains_wait_status_and_other_probes_continue(self):
        binary = self.context_binary("signal-context", "raise(SIGSYS); return 1;")
        report = self.run_probe("--path", str(self.root), binary=binary, code=1)
        check = self.records(report, "native")[0]
        self.assertEqual(check["signal"], signal.SIGSYS)
        self.assertIsNone(check["exit_code"])
        self.assertTrue(os.WIFSIGNALED(check["raw_wait_status"]))
        self.assertIn("native-context-call", check["stderr"])
        self.assertNotIn("native-context-returned", check["stderr"])
        self.assertNotIn("native-worker-_Exit", check["stderr"])
        self.assertEqual(self.records(report, "path")[0]["status"], "collected")

    def test_native_timeout_does_not_block_remaining_checks(self):
        binary = self.context_binary("slow-context", "sleep(5); return 1;")
        started = time.monotonic()
        report = self.run_probe("--path", str(self.root), binary=binary, code=1)
        check = self.records(report, "native")[0]
        self.assertTrue(check["timed_out"])
        self.assertIn("native-context-call", check["stderr"])
        self.assertNotIn("native-context-returned", check["stderr"])
        self.assertEqual(check["signal"], signal.SIGKILL)
        self.assertTrue(os.WIFSIGNALED(check["raw_wait_status"]))
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
            " st.st_mode=S_IFDIR|0711; assert(safe_ancestor(&st));\n"
            " st.st_uid=geteuid(); st.st_mode=S_IFDIR|0111; assert(safe_ancestor(&st));\n"
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
