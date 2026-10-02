use super::*;
use pretty_assertions::assert_eq;
use std::fs::File;
use std::os::unix::fs::PermissionsExt;

#[test]
fn resolves_native_probe_from_path_when_standard_locations_are_missing() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let native_true = dir.path().join("true");
    std::fs::write(&native_true, "#!/bin/sh\nexit 0\n")?;
    std::fs::set_permissions(&native_true, std::fs::Permissions::from_mode(0o755))?;
    let missing = dir.path().join("missing");
    assert_eq!(
        resolve_true_command_in(
            &[&missing],
            Some(dir.path().as_os_str().to_owned()),
            dir.path()
        )?,
        native_true,
    );
    Ok(())
}

#[test]
fn missing_or_nonexecutable_probe_command_is_not_a_namespace_failure() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let not_executable = dir.path().join("true");
    std::fs::write(&not_executable, "not executable")?;
    std::fs::set_permissions(&not_executable, std::fs::Permissions::from_mode(0o644))?;
    let error = resolve_true_command_in(
        &[&not_executable],
        Some(dir.path().as_os_str().to_owned()),
        dir.path(),
    )
    .expect_err("must require an executable probe command");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(
        error
            .to_string()
            .contains("probe command 'true' is unavailable")
    );
    Ok(())
}

#[test]
fn records_success_and_unsuccessful_exit_without_guessing_capability() -> io::Result<()> {
    for code in [0, 19] {
        let output = run(
            Command::new("/bin/sh")
                .args(["-c", &format!("printf out; printf err >&2; exit {code}")]),
            Duration::from_secs(2),
        )?;
        assert_eq!(output.status.code(), Some(code));
        assert_eq!(output.stdout, b"out");
        assert_eq!(output.stderr, b"err");
    }
    Ok(())
}

#[test]
fn startup_failure_preserves_errno() {
    let error = run(
        &mut Command::new("/definitely/not/a/sandbox-probe"),
        Duration::from_secs(1),
    )
    .expect_err("missing probe must not be available");
    assert_eq!(error.raw_os_error(), Some(libc::ENOENT));
}

#[test]
fn timeout_is_an_error_and_kills_the_probe_group() {
    let start = Instant::now();
    let error = run(
        Command::new("/bin/sh").args(["-c", "sleep 5 & wait"]),
        Duration::from_millis(50),
    )
    .expect_err("unfinished probe must not be available");
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn closed_output_does_not_make_a_running_probe_successful() {
    let error = run(
        Command::new("/bin/sh").args(["-c", "exec 1>&- 2>&-; sleep 5"]),
        Duration::from_millis(50),
    )
    .expect_err("pipe EOF is not successful exit");
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
}

#[test]
fn descendants_holding_output_open_do_not_delay_exit_observation() -> io::Result<()> {
    let start = Instant::now();
    let output = run(
        Command::new("/bin/sh").args(["-c", "printf diagnostic >&2; sleep 1 & exit 17"]),
        Duration::from_millis(250),
    )?;
    assert_eq!(output.status.code(), Some(17));
    assert_eq!(output.stderr, b"diagnostic");
    assert!(start.elapsed() < Duration::from_millis(750));
    Ok(())
}

#[test]
fn output_limit_is_an_error_instead_of_a_stalled_pipe() {
    for redirect in ["", ">&2"] {
        let error = run(
            Command::new("/bin/sh").args([
                "-c",
                &format!("while :; do printf '%1024s' x {redirect}; done"),
            ]),
            Duration::from_secs(2),
        )
        .expect_err("unbounded probe output must fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}

#[test]
fn wait_failure_preserves_errno() {
    let error = collect_output(None::<File>, None::<File>, Duration::from_secs(1), || {
        Err(io::Error::from_raw_os_error(libc::ECHILD))
    })
    .expect_err("failed observation cannot establish capability");
    assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
}
