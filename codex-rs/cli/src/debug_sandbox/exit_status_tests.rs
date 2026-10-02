use super::handle_debug_sandbox_exit_status;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;

#[test]
fn standalone_sandbox_exit_diagnostics_keep_native_status_and_redact_payloads() {
    const WORKER: &str = "CODEX_TEST_DEBUG_SANDBOX_EXIT_WORKER";
    const DIAGNOSTICS: &str = "CODEX_HARMONY_PROCESS_DIAGNOSTICS";
    const ARGUMENT_CANARY: &str = "private-debug-sandbox-argument";
    const ENVIRONMENT_CANARY: &str = "private-debug-sandbox-environment";

    if let Ok(case) = std::env::var(WORKER) {
        let script = match case.as_str() {
            "exit" => "exit 159",
            "signal" => "kill -TERM $$",
            _ => panic!("unknown test case"),
        };
        let output = Command::new("/bin/sh")
            .args(["-c", script, ARGUMENT_CANARY])
            .env("DEBUG_SANDBOX_PRIVATE_VALUE", ENVIRONMENT_CANARY)
            .output()
            .expect("run real shell child");
        if case == "exit" {
            assert_eq!(output.status.code(), Some(159));
            assert_eq!(output.status.signal(), None);
        } else {
            assert_eq!(output.status.code(), None);
            assert_eq!(output.status.signal(), Some(15));
        }
        handle_debug_sandbox_exit_status(output.status);
    }

    for flag in [None, Some("0"), Some("1")] {
        for (case, code, detail) in [
            (
                "exit",
                159,
                "exit_code=Some(159) signal=None raw_wait_status=40704",
            ),
            (
                "signal",
                143,
                "exit_code=None signal=Some(15) raw_wait_status=15",
            ),
        ] {
            let mut worker = Command::new(std::env::current_exe().expect("test executable"));
            worker
                .args([
                    "standalone_sandbox_exit_diagnostics_keep_native_status_and_redact_payloads",
                    "--nocapture",
                ])
                .env(WORKER, case)
                .env_remove(DIAGNOSTICS);
            if let Some(flag) = flag {
                worker.env(DIAGNOSTICS, flag);
            }
            let output = worker.output().expect("run isolated diagnostic worker");
            assert_eq!(output.status.code(), Some(code));
            let stderr = String::from_utf8(output.stderr).expect("UTF-8 diagnostics");
            if flag == Some("1") {
                assert!(stderr.contains("stage=debug-sandbox-wait"), "{stderr}");
                assert!(stderr.contains(detail), "{stderr}");
            } else {
                assert!(stderr.is_empty(), "disabled diagnostics: {stderr}");
            }
            for payload in [
                ARGUMENT_CANARY,
                ENVIRONMENT_CANARY,
                "exit 159",
                "kill -TERM",
            ] {
                assert!(!stderr.contains(payload), "unexpected payload: {stderr}");
            }
        }
    }
}
