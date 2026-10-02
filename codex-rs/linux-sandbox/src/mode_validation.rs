//! Validate requested sandbox modes before starting helpers or applying policy.

pub(crate) fn require_valid_mode(inner: bool, legacy: bool, full_disk_write: bool) {
    if let Err(error) = validate_mode(inner, legacy, full_disk_write) {
        eprintln!("error configuring Linux sandbox: {error}");
        std::process::exit(2);
    }
}

fn validate_mode(inner: bool, legacy: bool, full_disk_write: bool) -> Result<(), &'static str> {
    if inner && legacy {
        return Err("--apply-seccomp-then-exec is incompatible with --use-legacy-landlock");
    }
    if legacy && !full_disk_write {
        return Err(
            "filesystem-restricted execution requires bubblewrap to isolate app-server sockets; --use-legacy-landlock cannot enforce this policy. Use a working bubblewrap installation without the legacy option; the requested restrictions remain required",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_restrictions_cannot_use_legacy_mode() {
        let error = validate_mode(false, true, false).expect_err("socket isolation required");
        assert!(error.contains("isolate app-server sockets"));
        assert!(error.contains("requested restrictions remain required"));
        assert!(validate_mode(true, true, true).is_err());
        assert!(validate_mode(true, true, false).is_err());
    }

    #[test]
    fn supported_mode_combinations_remain_allowed() {
        for (inner, legacy, full_disk_write) in [
            (false, false, false),
            (false, false, true),
            (true, false, false),
            (true, false, true),
            (false, true, true),
        ] {
            require_valid_mode(inner, legacy, full_disk_write);
        }
    }

    #[test]
    fn invalid_mode_exits_two_with_explanation_and_without_panic() {
        const CHILD: &str = "CODEX_SANDBOX_MODE_TEST_CHILD";
        if let Ok(case) = std::env::var(CHILD) {
            require_valid_mode(case == "inner", true, case == "inner");
            panic!("invalid mode must not reach command execution");
        }
        for (case, message) in [
            ("restricted", "isolate app-server sockets"),
            ("inner", "incompatible with --use-legacy-landlock"),
        ] {
            let output = std::process::Command::new(
                std::env::current_exe().expect("test executable"),
            )
            .args([
                "--exact",
                "mode_validation::tests::invalid_mode_exits_two_with_explanation_and_without_panic",
                "--nocapture",
            ])
            .env(CHILD, case)
            .output()
            .expect("run validation subprocess");
            assert_eq!(output.status.code(), Some(2));
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains("error configuring Linux sandbox"),
                "{stderr}"
            );
            assert!(stderr.contains(message), "{stderr}");
            assert!(!stderr.contains("panicked"), "{stderr}");
        }
    }
}
