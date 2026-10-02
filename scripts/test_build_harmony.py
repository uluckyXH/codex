import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from build_harmony import TARGET
from build_harmony import build_environment
from build_harmony import native_sdk


class HarmonyBuildTests(unittest.TestCase):
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
        for name in ("clang", "clang++", "llvm-ar", "llvm-readelf"):
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
            "PKG_CONFIG_PATH": "/host/packages",
            "PKG_CONFIG_LIBDIR": "/host/lib",
            "CARGO_TARGET_DIR": "/old/build",
        }
        output = self.root / "build"
        env = build_environment(self.sdk, output, original)
        self.assertEqual(env["CC"], "/host/clang")
        self.assertEqual(env["CXX"], "/host/clang++")
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
