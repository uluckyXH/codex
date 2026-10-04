#!/usr/bin/env python3
"""Run the production snapshot wrapper and selected Rust tests without codex-core.

The wrapper, shell text helpers, constants and test functions are extracted verbatim.
Only dependency scaffolding is replaced: ShellType/Shell omit serde derives,
AbsolutePathBuf uses an already-absolute PathBuf, tempfile uses a private std-only
directory, and provider registry lookup asserts the absent marker used by these
fixtures before returning no provider keys. This is not a full cargo test.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess


def section(source, start, end):
    return source.split(start, 1)[1].split(end, 1)[0]


def function(source, name):
    match = re.search(
        rf"^(?:pub(?:\(crate\))? )?fn {re.escape(name)}\b[\s\S]*?^}}",
        source,
        re.MULTILINE,
    )
    if match is None:
        raise ValueError(f"missing function: {name}")
    return match.group(0)


def constant(source, name):
    match = re.search(
        rf"^(?:pub(?:\(crate\))? )?const {re.escape(name)}\b[\s\S]*?;\s*$",
        source,
        re.MULTILINE,
    )
    if match is None:
        raise ValueError(f"missing constant: {name}")
    return match.group(0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--rustc", required=True, type=Path)
    args = parser.parse_args()
    repo = args.repo.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    sources = {}

    def read(relative):
        data = (repo / relative).read_text()
        sources[relative] = hashlib.sha256(data.encode()).hexdigest()
        return data

    runtime = read("codex-rs/core/src/tools/runtimes/mod.rs")
    tests = read("codex-rs/core/src/tools/runtimes/mod_tests.rs")
    detect = read("codex-rs/shell-command/src/shell_detect.rs")
    shell = read("codex-rs/core/src/shell.rs")
    snapshot = read("codex-rs/shell-command/src/shell_snapshot.rs")
    protocol = read("codex-rs/protocol/src/shell_environment.rs")
    proxy = read("codex-rs/network-proxy/src/proxy.rs")
    credentials = read("codex-rs/network-proxy/src/credential_broker.rs")
    certs = read("codex-rs/network-proxy/src/certs.rs")
    exec_env = read("codex-rs/core/src/exec_env.rs")
    apply_patch = read("codex-rs/apply-patch/src/lib.rs")
    metrics = read("codex-rs/core-plugins/src/plugin_metrics_sidecar.rs")
    attribution = read("codex-rs/network-proxy/src/attribution.rs")
    parts = [
        "use std::collections::HashMap; use std::path::{Path,PathBuf};\n"
        "use std::os::unix::fs::PermissionsExt; use std::process::Command;",
        "#[derive(Debug,Clone,Copy,PartialEq,Eq)]\npub enum ShellType {"
        + section(detect, "pub enum ShellType {", "\n}")
        + "\n}",
        "pub struct Shell {" + section(shell, "pub struct Shell {", "\n}") + "\n}",
        "mod codex_shell_command { pub mod shell_detect { use crate::ShellType;\n"
        + function(detect, "detect_shell_type")
        + "\n}}",
        r"""
type AbsolutePathBuf = PathBuf;
trait PathBufExt { fn abs(self) -> PathBuf; }
impl PathBufExt for PathBuf {
    fn abs(self) -> PathBuf { assert!(self.is_absolute()); self }
}
struct TestTempDir(PathBuf);
impl TestTempDir { fn path(&self) -> &Path { &self.0 } }
impl Drop for TestTempDir {
    fn drop(&mut self) { std::fs::remove_dir_all(&self.0).expect("remove fixture"); }
}
fn tempdir() -> std::io::Result<TestTempDir> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!("snapshot-{}-{}",
        std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    Ok(TestTempDir(path))
}
mod codex_network_proxy {
    pub fn brokered_credential_env_keys(
        env: &std::collections::HashMap<String,String>
    ) -> impl Iterator<Item=&'static str> {
        assert!(!env.contains_key(crate::BROKERED_CREDENTIALS_ENV_KEY));
        std::iter::empty()
    }
}
""",
        section(
            runtime,
            "pub(crate) mod zsh_fork;",
            "pub(crate) fn exec_env_for_sandbox_permissions",
        ),
        section(
            runtime,
            "#[cfg(unix)]\nfn prepend_path_entry",
            "#[cfg(unix)]\npub(crate) fn apply_package_path_prepend",
        ),
    ]
    # Restore the function prefix excluded by the previous source boundary.
    parts[-1] = "fn prepend_path_entry" + parts[-1]
    constants = [
        (protocol, "CODEX_SESSION_ID_ENV_VAR"),
        (protocol, "CODEX_THREAD_ID_ENV_VAR"),
        (protocol, "CODEX_EXEC_SERVER_NOISE_AUTH_TOKEN_ENV_VAR"),
        (protocol, "OPENAI_FEDERATION_RULE_ID_ENV_VAR"),
        (protocol, "OPENAI_IDENTITY_TOKEN_FILE_ENV_VAR"),
        (protocol, "OPENAI_WORKLOAD_IDENTITY_CONTEXT_ENV_VAR"),
        (protocol, "NON_INHERITABLE_ENV_VARS"),
        (exec_env, "CODEX_VERSION_ENV_VAR"),
        (exec_env, "CODEX_PERMISSION_PROFILE_ENV_VAR"),
        (apply_patch, "CODEX_APPLY_PATCH_PRESERVE_LINE_ENDINGS_ENV_VAR"),
        (metrics, "PLUGIN_METRICS_OUTPUT_ENV_VAR"),
        (credentials, "CREDENTIAL_BROKER_ACTIVE_ENV_KEY"),
        (credentials, "BROKERED_CREDENTIALS_ENV_KEY"),
        (proxy, "PROXY_ACTIVE_ENV_KEY"),
        (proxy, "ALLOW_LOCAL_BINDING_ENV_KEY"),
        (proxy, "ELECTRON_GET_USE_PROXY_ENV_KEY"),
        (proxy, "NODE_USE_ENV_PROXY_ENV_KEY"),
        (proxy, "GIT_SSH_COMMAND_ENV_KEY"),
        (proxy, "PROXY_ENV_KEYS"),
        (proxy, "PROXY_GIT_SSH_COMMAND_ENV_KEY"),
        (proxy, "CODEX_PROXY_GIT_SSH_COMMAND_MARKER"),
        (attribution, "PROXY_ATTRIBUTION_TOKEN_ENV_KEY"),
        (certs, "CUSTOM_CA_ENV_KEYS"),
    ]
    parts.extend(constant(text, name) for text, name in constants)
    parts.extend(
        [
            function(protocol, "is_non_inheritable_env_var"),
            function(snapshot, "posix_env_path_expansion_function"),
            "pub(crate) fn maybe_wrap_shell_lc_with_snapshot"
            + section(
                runtime,
                "pub(crate) fn maybe_wrap_shell_lc_with_snapshot",
                "#[cfg(test)]\nmod prepare_powershell_command_tests",
            ),
            function(tests, "shell_with_snapshot"),
            function(tests, "single_launch_sh"),
        ]
    )
    selected_tests = [
        "ohos_snapshot_reuses_sh_and_preserves_environment_arguments_and_exit",
        "ohos_snapshot_reuse_preserves_shell_options",
        "ohos_snapshot_reuse_requires_matching_sh_and_snapshot",
        "ohos_snapshot_reuse_preserves_brokered_credentials_and_startup_cleanup",
        "maybe_wrap_shell_lc_with_snapshot_bootstraps_in_user_shell",
        "maybe_wrap_shell_lc_with_snapshot_escapes_single_quotes",
        "maybe_wrap_shell_lc_with_snapshot_uses_bash_bootstrap_shell",
        "maybe_wrap_shell_lc_with_snapshot_uses_sh_bootstrap_shell",
        "maybe_wrap_shell_lc_with_snapshot_preserves_trailing_args",
        "maybe_wrap_shell_lc_with_snapshot_reuses_brokered_session_zsh",
        "maybe_wrap_shell_lc_with_snapshot_restores_explicit_override_precedence",
        "maybe_wrap_shell_lc_with_snapshot_keeps_snapshot_path_without_override",
        "maybe_wrap_shell_lc_with_snapshot_applies_explicit_path_override",
    ]
    parts.extend("#[test]\n" + function(tests, name) for name in selected_tests)
    harness = "\n\n".join(parts) + "\n"
    source_path = output / "snapshot_wrapper_harness.rs"
    source_path.write_text(harness)
    metadata = {
        "sources_sha256": sources,
        "harness_sha256": hashlib.sha256(harness.encode()).hexdigest(),
        "test_functions": selected_tests,
        "limitations": __doc__,
    }
    (output / "source_metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    env = os.environ.copy()
    temp = output / "tmp"
    temp.mkdir(exist_ok=True)
    env["TMPDIR"] = str(temp)
    binary = output / "snapshot-wrapper-tests"
    subprocess.run(
        [
            str(args.rustc),
            "--edition=2024",
            "--test",
            "-Adead_code",
            str(source_path),
            "-o",
            str(binary),
        ],
        env=env,
        check=True,
    )
    subprocess.run([str(binary), "--nocapture"], env=env, check=True)


if __name__ == "__main__":
    main()
