//! Opt-in process diagnostics contain only static stages, PIDs, and OS status.
//! Never print command arguments, environment entries, or file contents here.

use std::io;
use std::process::ExitStatus;

pub(crate) const ENV_VAR: &str = "CODEX_HARMONY_PROCESS_DIAGNOSTICS";

pub(crate) fn enabled() -> bool {
    std::env::var_os(ENV_VAR).is_some_and(|value| value == "1")
}

#[cfg(target_os = "linux")]
pub(crate) fn stage(enabled: bool, stage: &'static str) {
    if enabled {
        eprintln!("[codex-process] pid={} stage={stage}", std::process::id());
    }
}

/// Describe the native wait result before compatibility exit-code normalization.
/// In particular, an explicit exit 159 is different from a signal termination.
pub fn describe_exit_status(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        format!(
            "exit_code={:?} signal={:?} raw_wait_status={} core_dumped={}",
            status.code(),
            status.signal(),
            status.into_raw(),
            status.core_dumped()
        )
    }
    #[cfg(not(unix))]
    {
        format!("exit_code={:?} native_status={status}", status.code())
    }
}

pub(crate) fn wait_status(stage: &'static str, status: ExitStatus) {
    if enabled() {
        eprintln!(
            "[codex-process] pid={} stage={stage} {}",
            std::process::id(),
            describe_exit_status(status)
        );
    }
}

pub(crate) fn failure(enabled: bool, stage: &'static str, error: &io::Error) {
    if enabled {
        eprintln!(
            "[codex-process] pid={} stage={stage} errno={:?} kind={:?}",
            std::process::id(),
            error.raw_os_error(),
            error.kind()
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn explicit_159_does_not_claim_a_signal() {
        let detail = describe_exit_status(ExitStatus::from_raw(159 << 8));
        assert!(detail.contains("exit_code=Some(159) signal=None"));
        assert!(detail.contains("raw_wait_status=40704"));
    }

    #[test]
    fn signal_is_kept_separate_from_an_explicit_exit() {
        let detail = describe_exit_status(ExitStatus::from_raw(31));
        assert!(detail.contains("exit_code=None signal=Some(31)"));
        assert!(detail.contains("raw_wait_status=31"));
        assert!(!detail.contains("SIGSYS"));
    }
}
