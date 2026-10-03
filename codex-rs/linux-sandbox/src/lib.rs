//! Linux sandbox helper entry point.
//!
//! On Linux, `codex-linux-sandbox` applies:
//! - in-process restrictions (`no_new_privs` + seccomp), and
//! - bubblewrap for filesystem isolation.
#[cfg(target_os = "linux")]
mod bazel_bwrap;
#[cfg(target_os = "linux")]
mod bundled_bwrap;
#[cfg(target_os = "linux")]
mod bwrap;
#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
mod daemon_mounts;
#[cfg(target_os = "linux")]
mod exec_util;
#[cfg(target_os = "linux")]
mod fd_mount;
#[cfg(target_os = "linux")]
mod landlock;
#[cfg(target_os = "linux")]
mod launcher;
#[cfg(target_os = "linux")]
mod linux_run_main;
#[cfg(any(target_os = "linux", test))]
mod mode_validation;
#[cfg(target_os = "linux")]
mod proxy_lifecycle;
#[cfg(target_os = "linux")]
mod proxy_routing;
#[cfg(target_os = "linux")]
mod wslg;

// The OHOS main entry shares this crate's existing platform dependency. Keep
// helper dispatch separate: re-executed helpers must retain their target's
// environment and must not become non-dumpable before namespace setup.
#[cfg(target_env = "ohos")]
pub use codex_process_hardening::pre_main_hardening;

/// Exit status returned when bundled bubblewrap fails digest verification.
#[cfg(target_os = "linux")]
pub const BUNDLED_BWRAP_DIGEST_VERIFICATION_FAILURE_EXIT_CODE: i32 = 8;

#[cfg(target_os = "linux")]
pub fn run_main() -> ! {
    linux_run_main::run_main();
}

#[cfg(not(target_os = "linux"))]
pub fn run_main() -> ! {
    panic!("codex-linux-sandbox is only supported on Linux");
}

#[cfg(any(target_env = "ohos", test))]
mod ohos_runtime_mounts;
