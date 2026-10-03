import json
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from build_harmony import TARGET
from build_harmony import build_environment
from build_harmony import hnp_alias_build_environment
from build_harmony import native_sdk
from build_harmony import runtime_base_contract
from build_harmony import runtime_build_environment


class HarmonyBuildTests(unittest.TestCase):
    def test_hnp_alias_digest_cannot_be_inherited_or_used_for_other_profiles(self):
        original = {"CODEX_HNP_ALIAS_SHA256": "a" * 64, "UNRELATED": "keep"}
        self.assertEqual(
            hnp_alias_build_environment(original, "strict", None), {"UNRELATED": "keep"}
        )
        env = hnp_alias_build_environment(original, "hnp-debug", "b" * 64)
        self.assertEqual(env["CODEX_HNP_ALIAS_SHA256"], "b" * 64)
        for profile, digest in (
            ("strict", "b" * 64),
            ("hdc-debug", "b" * 64),
            ("hnp-debug", "B" * 64),
            ("hnp-debug", "b" * 63),
            ("hnp-debug", "b" * 64 + "\n"),
        ):
            with (
                self.subTest(profile=profile, digest=digest),
                self.assertRaises(ValueError),
            ):
                hnp_alias_build_environment(original, profile, digest)

    def test_runtime_profile_must_be_explicit_and_cannot_leak_from_shell(self):
        original = {
            "CODEX_OHOS_RUNTIME_BASE": "/data/local/tmp/cdx",
            "CODEX_OHOS_RUNTIME_PROFILE": "hdc-debug",
            "CODEX_OHOS_RUNTIME_UID": "20020059",
            "UNRELATED": "keep",
        }
        env = runtime_build_environment(original, None)
        self.assertNotIn("CODEX_OHOS_RUNTIME_BASE", env)
        self.assertNotIn("CODEX_OHOS_RUNTIME_UID", env)
        self.assertEqual(env["CODEX_OHOS_RUNTIME_PROFILE"], "strict")
        self.assertEqual(env["UNRELATED"], "keep")
        self.assertEqual(original["CODEX_OHOS_RUNTIME_PROFILE"], "hdc-debug")
        env = runtime_build_environment(original, "/data/local/tmp/cdx", "hdc-debug")
        self.assertEqual(env["CODEX_OHOS_RUNTIME_PROFILE"], "hdc-debug")
        self.assertEqual(env["CODEX_OHOS_RUNTIME_BASE"], "/data/local/tmp/cdx")
        for profile, base in (
            ("unknown", "/data/local/tmp/cdx"),
            ("hdc-debug", None),
            ("hdc-debug", "/data/storage/el2/base/files"),
            ("hdc-debug", "/data/local/tmp/cdx-other"),
        ):
            with (
                self.subTest(profile=profile, base=base),
                self.assertRaises(ValueError),
            ):
                runtime_build_environment(original, base, profile)

    def test_hnp_debug_requires_explicit_application_identity_and_exact_base(self):
        base = "/data/storage/el2/base/files/r"
        env = runtime_build_environment({}, base, "hnp-debug", 20020059)
        self.assertEqual(env["CODEX_OHOS_RUNTIME_UID"], "20020059")
        self.assertEqual(env["CODEX_OHOS_RUNTIME_BASE"], base)
        self.assertEqual(env["CODEX_OHOS_RUNTIME_PROFILE"], "hnp-debug")
        self.assertEqual(len(base.encode()) + len("/cffffffff/s/" + "f" * 64) + 1, 108)
        for uid in (None, 0, 2000, 9999, True, "20020059", 0x100000000):
            with self.subTest(uid=uid), self.assertRaises(ValueError):
                runtime_build_environment({}, base, "hnp-debug", uid)
        for invalid_base in (None, "/data/local/tmp/cdx", base + "x"):
            with self.subTest(base=invalid_base), self.assertRaises(ValueError):
                runtime_build_environment({}, invalid_base, "hnp-debug", 20020059)
        for profile, other_base in (
            ("strict", base),
            ("hdc-debug", "/data/local/tmp/cdx"),
        ):
            with self.subTest(profile=profile), self.assertRaises(ValueError):
                runtime_build_environment({}, other_base, profile, 20020059)

    def test_runtime_contract_keeps_target_path_and_full_socket_identity(self):
        self.assertEqual(
            runtime_base_contract("/data/storage/el2/base/files"),
            "/data/storage/el2/base/files",
        )
        self.assertEqual(runtime_base_contract("/" + "x" * 29), "/" + "x" * 29)
        for value in (
            "/",
            "relative",
            "/a/../b",
            "/a/./b",
            "/a//b",
            "/a/",
            "/a\nb",
            "/a\x00b",
            "/" + "x" * 30,
            "/" + "中" * 11,
        ):
            with (
                self.subTest(value=value),
                self.assertRaises(argparse.ArgumentTypeError),
            ):
                runtime_base_contract(value)

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.sdk = self.root / "SDK with space ' quotation/openharmony/native"
        (self.sdk / "sysroot/usr/include").mkdir(parents=True)
        (self.sdk / "sysroot/usr/include/spawn.h").touch()
        (self.sdk / "sysroot/usr/lib/aarch64-linux-ohos").mkdir(parents=True)
        (self.sdk / "sysroot/usr/lib/aarch64-linux-ohos/libc.so").touch()
        (self.sdk / "llvm/bin").mkdir(parents=True)
        for name in ("clang", "clang++", "llvm-ar", "llvm-ranlib", "llvm-readelf"):
            path = self.sdk / "llvm/bin" / name
            path.write_text(
                f"#!{sys.executable}\n"
                "import json, sys\n"
                "print(json.dumps(sys.argv[1:]))\n"
            )
            path.chmod(0o755)

    def test_accepts_sdk_version_and_native_directories(self):
        for root in (self.sdk, self.sdk.parent, self.sdk.parent.parent):
            self.assertEqual(native_sdk(root), self.sdk.resolve())

    def test_rejects_incomplete_sdk(self):
        (self.sdk / "llvm/bin/clang++").unlink()
        with self.assertRaisesRegex(ValueError, "clang"):
            native_sdk(self.sdk)

    def test_rejects_sdk_without_target_libraries(self):
        (self.sdk / "sysroot/usr/lib/aarch64-linux-ohos/libc.so").unlink()
        with self.assertRaisesRegex(ValueError, "libc.so"):
            native_sdk(self.sdk)

    def test_rejects_sdk_without_target_archive_indexer(self):
        (self.sdk / "llvm/bin/llvm-ranlib").unlink()
        with self.assertRaisesRegex(ValueError, "llvm-ranlib"):
            native_sdk(self.sdk)

    def test_wrapper_preserves_arguments_and_quoted_paths(self):
        output = self.root / "output with spaces"
        env = build_environment(self.sdk, output, dict(os.environ))
        cc = env["CC_aarch64_unknown_linux_ohos"]
        literal = f"$(touch {self.root / 'unexpected'})"
        arguments = ["-o", "program with spaces", literal, "a'b", ""]
        actual = json.loads(subprocess.check_output([cc, *arguments], text=True))
        self.assertEqual(
            actual,
            [
                "--target=aarch64-linux-ohos",
                f"--sysroot={self.sdk / 'sysroot'}",
                "-D__MUSL__",
                *arguments,
            ],
        )
        self.assertFalse((self.root / "unexpected").exists())

    def test_preserves_host_compiler_and_isolates_target_pkg_config(self):
        original = {
            "CC": "/host/clang",
            "CXX": "/host/clang++",
            "AR": "/host/ar",
            "RANLIB": "/host/ranlib",
            "PKG_CONFIG_PATH": "/host/packages",
            "PKG_CONFIG_LIBDIR": "/host/lib",
            "CARGO_TARGET_DIR": "/old/build",
        }
        output = self.root / "build"
        env = build_environment(self.sdk, output, original)
        self.assertEqual(env["CC"], "/host/clang")
        self.assertEqual(env["CXX"], "/host/clang++")
        self.assertEqual(env["AR"], "/host/ar")
        self.assertEqual(env["RANLIB"], "/host/ranlib")
        for suffix in (TARGET, TARGET.replace("-", "_")):
            self.assertEqual(env[f"AR_{suffix}"], str(self.sdk / "llvm/bin/llvm-ar"))
            self.assertEqual(
                env[f"RANLIB_{suffix}"], str(self.sdk / "llvm/bin/llvm-ranlib")
            )
        self.assertEqual(env[f"PKG_CONFIG_PATH_{TARGET}"], "")
        self.assertNotIn("/host/", env[f"PKG_CONFIG_LIBDIR_{TARGET}"])
        self.assertEqual(env["CARGO_TARGET_DIR"], str(output / "target"))
        self.assertEqual(env["TMPDIR"], str(output / "tmp"))
        self.assertEqual(original["CARGO_TARGET_DIR"], "/old/build")
        self.assertNotIn("TMPDIR", original)

    def test_preserves_explicit_target_dependency_directories(self):
        key = "PKG_CONFIG_LIBDIR_aarch64_unknown_linux_ohos"
        env = build_environment(self.sdk, self.root / "build", {key: "/target/lib"})
        self.assertEqual(env[key], "/target/lib")
        self.assertEqual(env[f"PKG_CONFIG_LIBDIR_{TARGET}"], "/target/lib")

    def test_uses_sdk_cmake_without_changing_host_toolchain(self):
        cmake = self.sdk / "build-tools/cmake/bin/cmake"
        cmake.parent.mkdir(parents=True)
        cmake.touch()
        toolchain = self.sdk / "build/cmake/ohos.toolchain.cmake"
        toolchain.parent.mkdir(parents=True)
        toolchain.touch()
        env = build_environment(
            self.sdk,
            self.root / "build",
            {"PATH": "/host/bin", "CMAKE_TOOLCHAIN_FILE": "/host/toolchain.cmake"},
        )
        self.assertEqual(env["OHOS_NDK_HOME"], str(self.sdk.parent))
        self.assertEqual(env["OHOS_SDK_NATIVE"], str(self.sdk))
        self.assertEqual(env[f"CMAKE_{TARGET}"], str(cmake))
        self.assertEqual(env[f"CMAKE_TOOLCHAIN_FILE_{TARGET}"], str(toolchain))
        self.assertEqual(env["CMAKE_TOOLCHAIN_FILE"], "/host/toolchain.cmake")
        self.assertEqual(env["PATH"], str(cmake.parent) + os.pathsep + "/host/bin")


if __name__ == "__main__":
    unittest.main()
