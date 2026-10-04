#!/usr/bin/env python3
"""直接引用 TUI 源码验证更新策略，不编译或执行登录、认证及完整应用。"""
from pathlib import Path
import os
import shutil
import subprocess
import sys

report = Path(__file__).resolve().parents[1]
root = report.parents[3]
output = root / ".harmony-build/测试批次" / report.name / "策略验证"
output.mkdir(exist_ok=True)
manifest = output / "Cargo.toml"
manifest.write_text(f"""[package]
name = "harmony-tui-policy-verification"
version = "0.0.0"
edition = "2024"

[workspace]

[lib]
path = "策略验证.rs"

[dependencies]
codex-install-context = {{ path = "{root}/codex-rs/install-context" }}
codex-utils-absolute-path = {{ path = "{root}/codex-rs/utils/absolute-path" }}
shlex = "=1.3.0"

[target.'cfg(not(target_env = "ohos"))'.dependencies]
webbrowser = "=1.2.4"

[dev-dependencies]
pretty_assertions = "=1.4.1"
""")
(output / "策略验证.rs").write_text(f"""
#![allow(dead_code)]
#[path = "{root}/codex-rs/tui/src/update_action.rs"]
mod update_action;
#[path = "{root}/codex-rs/tui/src/external_browser.rs"]
mod external_browser;

#[cfg(all(test, target_env = "ohos"))]
mod harmony_tests {{
    #[test]
    fn update_and_browser_backends_report_unsupported() {{
        assert!(super::update_action::update_unavailable_reason().is_some());
        assert!(super::update_action::UpdateAction::from_install_context(
            &codex_install_context::InstallContext {{
                method: codex_install_context::InstallMethod::Npm,
                package_layout: None,
            }},
        ).is_none());
        let error = super::external_browser::open("https://example.invalid/help").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("https://example.invalid/help"));
    }}
}}
""")
cargo = ["rustup", "run", "1.95.0", "cargo"]
if not (output / "Cargo.lock").exists():
    shutil.copyfile(root / "codex-rs/Cargo.lock", output / "Cargo.lock")
    subprocess.run(cargo + ["generate-lockfile", "--manifest-path", str(manifest), "--offline"], check=True)
if sys.argv[1] == "host":
    command = cargo + ["test", "--manifest-path", str(manifest), "--offline", "--locked", "--target", "aarch64-apple-darwin", "--lib"]
    env = os.environ.copy()
elif sys.argv[1] == "ohos":
    sys.path.insert(0, str(root / "scripts"))
    from build_harmony import build_environment, native_sdk
    sdk = native_sdk(Path("/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native"))
    env = build_environment(sdk, output / "鸿蒙检查", dict(os.environ))
    command = cargo + ["check", "--manifest-path", str(manifest), "--offline", "--locked", "--release", "--tests", "--target", "aarch64-unknown-linux-ohos"]
else:
    raise SystemExit("用法：隔离策略验证.py host|ohos")
print("引用原始 TUI 源文件；隔离清单和锁文件仅在本批外置盘输出目录生成。", flush=True)
subprocess.run(command, env=env, check=True)
