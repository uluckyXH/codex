#[cfg(target_os = "linux")]
use crate::policy_transforms::should_require_platform_sandbox;
#[cfg(target_os = "linux")]
use codex_protocol::models::PermissionProfile;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;
use std::time::Duration;

const SYSTEM_BWRAP_PROGRAM: &str = "bwrap";
const MISSING_BWRAP_WARNING: &str = concat!(
    "Codex could not find bubblewrap on PATH. ",
    "Install bubblewrap with your OS package manager. ",
    "See the sandbox prerequisites: ",
    "https://developers.openai.com/codex/concepts/sandboxing#prerequisites. ",
    "Codex will try packaged bubblewrap if available; restricted commands cannot run without a working sandbox.",
);
const USER_NAMESPACE_WARNING: &str =
    "Codex's Linux sandbox uses bubblewrap and needs access to create user namespaces.";
pub(crate) const WSL1_BWRAP_WARNING: &str = concat!(
    "Codex's Linux sandbox uses bubblewrap, which is not supported on WSL1 ",
    "because WSL1 cannot create the required user namespaces. ",
    "Use WSL2 for sandboxed shell commands."
);
const USER_NAMESPACE_FAILURES: [&str; 4] = [
    "loopback: Failed RTM_NEWADDR",
    "loopback: Failed RTM_NEWLINK",
    "setting up uid map: Permission denied",
    "No permissions to create a new namespace",
];
const SYSTEM_BWRAP_PROBE_TIMEOUT: Duration = Duration::from_millis(500);

#[cfg(target_os = "linux")]
pub fn system_bwrap_warning(permission_profile: &PermissionProfile) -> Option<String> {
    if !should_warn_about_system_bwrap(permission_profile) {
        return None;
    }

    let system_bwrap_path = find_system_bwrap_in_path();
    #[cfg(target_env = "ohos")]
    {
        let bundled = packaged_bwrap_candidate(
            codex_install_context::InstallContext::current(),
            std::env::current_exe().ok().as_deref(),
        );
        harmony_bwrap_warning(
            system_bwrap_path.as_deref(),
            bundled.as_deref(),
            |path| crate::probe::run(Command::new(path).arg("--help"), SYSTEM_BWRAP_PROBE_TIMEOUT),
            |path| system_bwrap_user_namespace_probe(path, SYSTEM_BWRAP_PROBE_TIMEOUT),
        )
    }
    #[cfg(not(target_env = "ohos"))]
    system_bwrap_warning_for_path(system_bwrap_path.as_deref())
}

#[cfg(target_os = "linux")]
fn should_warn_about_system_bwrap(permission_profile: &PermissionProfile) -> bool {
    let (file_system_policy, network_policy) = permission_profile.to_runtime_permissions();
    should_require_platform_sandbox(
        &file_system_policy,
        network_policy,
        /*has_managed_network_requirements*/ false,
    )
}

fn system_bwrap_warning_for_path(system_bwrap_path: Option<&Path>) -> Option<String> {
    if is_wsl1() {
        return Some(WSL1_BWRAP_WARNING.to_string());
    }

    let Some(system_bwrap_path) = system_bwrap_path else {
        return Some(MISSING_BWRAP_WARNING.to_string());
    };

    warning_for_probe_result(system_bwrap_user_namespace_probe(
        system_bwrap_path,
        SYSTEM_BWRAP_PROBE_TIMEOUT,
    ))
}

fn warning_for_probe_result(result: std::io::Result<Output>) -> Option<String> {
    match result {
        Ok(output) if output.status.success() => None,
        Ok(output) if is_user_namespace_failure(&output) => {
            Some(USER_NAMESPACE_WARNING.to_string())
        }
        Ok(output) => Some(format!(
            "Codex could not verify bubblewrap namespace support: probe exited with {}: {}. Restricted execution still requires a working sandbox.",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => Some(format!(
            "Codex could not verify bubblewrap namespace support: {error}. Restricted execution still requires a working sandbox."
        )),
    }
}

fn system_bwrap_user_namespace_probe(
    system_bwrap_path: &Path,
    timeout: Duration,
) -> std::io::Result<Output> {
    let probe_command = crate::probe::resolve_true_command()?;
    crate::probe::run(
        Command::new(system_bwrap_path)
            .args(["--unshare-user", "--unshare-net", "--ro-bind", "/", "/"])
            .arg(probe_command),
        timeout,
    )
}

pub(crate) fn is_wsl1() -> bool {
    std::fs::read_to_string("/proc/version")
        .is_ok_and(|proc_version| proc_version_indicates_wsl1(&proc_version))
}

fn proc_version_indicates_wsl1(proc_version: &str) -> bool {
    let proc_version = proc_version.to_ascii_lowercase();
    let mut remaining = proc_version.as_str();
    while let Some(marker) = remaining.find("wsl") {
        let version_start = marker + "wsl".len();
        let version_digits: String = remaining[version_start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(version) = version_digits.parse::<u32>() {
            return version == 1;
        }
        remaining = &remaining[version_start..];
    }

    proc_version.contains("microsoft") && !proc_version.contains("microsoft-standard")
}

fn is_user_namespace_failure(output: &Output) -> bool {
    let stderr = String::from_utf8_lossy(&output.stderr);
    USER_NAMESPACE_FAILURES
        .iter()
        .any(|failure| stderr.contains(failure))
}

#[cfg(target_os = "linux")]
pub fn find_system_bwrap_in_path() -> Option<PathBuf> {
    let search_path = std::env::var_os("PATH")?;
    let cwd = std::env::current_dir().ok()?;
    find_system_bwrap_in_search_paths(std::env::split_paths(&search_path), &cwd)
}

fn find_system_bwrap_in_search_paths(
    search_paths: impl IntoIterator<Item = PathBuf>,
    cwd: &Path,
) -> Option<PathBuf> {
    let search_path = std::env::join_paths(search_paths).ok()?;
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let cwd_is_root = cwd.parent().is_none();
    which::which_in_all(SYSTEM_BWRAP_PROGRAM, Some(search_path), &cwd)
        .ok()?
        .find_map(|path| {
            let path = std::fs::canonicalize(path).ok()?;
            if !cwd_is_root && path.starts_with(&cwd) {
                None
            } else {
                Some(path)
            }
        })
}

#[cfg(test)]
#[path = "bwrap_tests.rs"]
mod tests;

/// Inspect candidates only. This never executes a package resource or bypasses
/// the launcher's digest verification. Presence is not proof of usable sandboxing.
#[cfg(target_os = "linux")]
pub fn bwrap_resource_diagnostics() -> Vec<String> {
    let system = find_system_bwrap_in_path();
    let packaged = packaged_bwrap_candidate(
        codex_install_context::InstallContext::current(),
        std::env::current_exe().ok().as_deref(),
    );
    vec![
        format!("bubblewrap PATH candidate: {}", candidate_label(system.as_deref())),
        format!("bubblewrap packaged candidate: {}", candidate_label(packaged.as_deref())),
        "bubblewrap selection: PATH candidate supporting --as-pid-1 and --perms first, otherwise packaged candidate; launcher checks packaged digest when configured".to_owned(),
        "bubblewrap readiness: candidates inspected only; loading, namespaces, seccomp and package digest not verified by this report".to_owned(),
    ]
}

fn candidate_label(path: Option<&Path>) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_else(|| "not found".to_owned())
}

fn packaged_bwrap_candidate(
    context: &codex_install_context::InstallContext,
    exe: Option<&Path>,
) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let package = context
        .bundled_resource("bwrap")
        .map(|path| path.into_path_buf());
    // Match the ordinary on-disk launcher's package and legacy path order. Bazel
    // runfiles are a build-system path, not part of the HarmonyOS install layout.
    let mut candidates = package.into_iter().collect::<Vec<_>>();
    if let Some(directory) = exe.and_then(Path::parent) {
        candidates.push(directory.join("codex-resources/bwrap"));
        if let Some(parent) = directory.parent() {
            candidates.push(parent.join("codex-resources/bwrap"));
        }
        candidates.push(directory.join("bwrap"));
    }
    candidates.into_iter().find(|path| {
        path.metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    })
}

#[cfg(any(target_env = "ohos", test))]
fn harmony_bwrap_warning(
    system: Option<&Path>,
    packaged: Option<&Path>,
    help_probe: impl FnOnce(&Path) -> std::io::Result<Output>,
    namespace_probe: impl FnOnce(&Path) -> std::io::Result<Output>,
) -> Option<String> {
    let Some(system) = system else {
        return packaged.is_none().then(|| "HarmonyOS sandbox: no executable bubblewrap candidate was found on PATH or in the Codex package. Repair the signed Codex package; restricted commands remain blocked until the sandbox works.".to_owned());
    };
    let capability_failure = match help_probe(system) {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            ["--as-pid-1", "--perms"]
                .iter()
                .find(|flag| !stdout.contains(**flag) && !stderr.contains(**flag))
                .map(|flag| format!("missing required capability {flag}"))
        }
        Ok(output) => Some(format!(
            "capability probe failed: {}",
            codex_utils_pty::describe_exit_status(output.status)
        )),
        Err(error) => Some(format!("capability probe could not run: {error}")),
    };
    if let Some(failure) = capability_failure {
        return Some(format!(
            "HarmonyOS sandbox: PATH bubblewrap {}: {failure}. Packaged candidate: {} (loading, digest and namespace support unverified). Restricted execution still requires a working sandbox.",
            system.display(),
            candidate_label(packaged)
        ));
    }
    match namespace_probe(system) {
        Ok(output) if output.status.success() => None,
        Ok(output) => Some(format!(
            "HarmonyOS sandbox: selected PATH bubblewrap {} passed the capability probe but its namespace probe failed: {}; {}. Restricted execution still requires a working sandbox.",
            system.display(),
            codex_utils_pty::describe_exit_status(output.status),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => Some(format!(
            "HarmonyOS sandbox: selected PATH bubblewrap {} passed the capability probe but its namespace probe could not run: {error}. Restricted execution still requires a working sandbox.",
            system.display()
        )),
    }
}
