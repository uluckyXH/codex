//! Host coverage for cleanup decisions and real Unix descriptor flags.
//! Range/procfs failures are injected; this does not exercise OHOS syscalls.

use std::io;
use std::os::fd::AsRawFd;
use std::os::fd::FromRawFd;
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;

use pretty_assertions::assert_eq;

use super::cleanup_with_fallback;
use super::mark_cloexec;
use super::mark_proc_entries;

fn entry(name: &[u8]) -> Vec<u8> {
    let length = (20 + name.len()).next_multiple_of(8);
    let mut bytes = vec![0; length];
    bytes[16..18].copy_from_slice(&(length as u16).to_ne_bytes());
    bytes[19..19 + name.len()].copy_from_slice(name);
    bytes
}

#[test]
fn range_cleanup_preserves_stdio_and_unsorted_duplicate_allowlist() -> io::Result<()> {
    let mut ranges = Vec::new();
    cleanup_with_fallback(
        &[9, 3, 7, 7, 2, -1],
        |first, last| {
            ranges.push((first, last));
            true
        },
        || panic!("successful range cleanup must not use procfs"),
    )?;
    assert_eq!(ranges, [(4, 6), (8, 8), (10, u32::MAX)]);
    Ok(())
}

#[test]
fn partial_range_failure_uses_complete_fallback() -> io::Result<()> {
    let mut calls = 0;
    let mut fallback_called = false;
    cleanup_with_fallback(
        &[7],
        |_, _| {
            calls += 1;
            calls == 1
        },
        || {
            fallback_called = true;
            Ok(())
        },
    )?;
    assert_eq!(calls, 2);
    assert!(fallback_called);
    Ok(())
}

#[test]
fn fallback_preserves_failure_errno() {
    for errno in [libc::EACCES, libc::EPERM, libc::EIO] {
        let error = cleanup_with_fallback(
            &[],
            |_, _| false,
            || Err(io::Error::from_raw_os_error(errno)),
        )
        .expect_err("failed enumeration must fail cleanup");
        assert_eq!(error.raw_os_error(), Some(errno));
    }
}

#[test]
fn cleanup_marks_unrelated_fds_without_closing_report_or_preserved_fds() -> anyhow::Result<()> {
    let file = tempfile::tempfile()?;
    let duplicate = |operation| -> io::Result<OwnedFd> {
        // SAFETY: fcntl returns a new owned fd and does not replace an existing one.
        let fd = unsafe { libc::fcntl(file.as_raw_fd(), operation, 3) };
        if fd == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    };
    let unrelated = duplicate(libc::F_DUPFD)?;
    let preserved = duplicate(libc::F_DUPFD)?;
    let report = duplicate(libc::F_DUPFD_CLOEXEC)?;
    let mut entries = entry(b".");
    entries.extend(entry(b".."));
    for fd in [
        0,
        1,
        2,
        unrelated.as_raw_fd(),
        preserved.as_raw_fd(),
        report.as_raw_fd(),
    ] {
        entries.extend(entry(fd.to_string().as_bytes()));
    }
    mark_proc_entries(
        &entries,
        &[preserved.as_raw_fd(), preserved.as_raw_fd()],
        true,
    )?;
    let flags = |fd| unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert_eq!(flags(unrelated.as_raw_fd()), libc::FD_CLOEXEC);
    assert_eq!(flags(preserved.as_raw_fd()), 0);
    // The CLOEXEC report fd stays open until exec, so failed setup can report errno.
    assert_eq!(flags(report.as_raw_fd()), libc::FD_CLOEXEC);
    assert_ne!(flags(file.as_raw_fd()), -1);
    Ok(())
}

#[test]
fn strict_cleanup_rejects_incomplete_or_unknown_proc_records() {
    let mut overlong = entry(b"3");
    overlong[16..18].copy_from_slice(&u16::MAX.to_ne_bytes());
    let mut unterminated = entry(b"3");
    unterminated[19..].fill(b'3');
    for bytes in [
        vec![0; 19],
        vec![0; 24],
        overlong,
        unterminated,
        entry(b"unknown"),
        entry(b"-1"),
        entry(b"2147483648"),
    ] {
        let error = mark_proc_entries(&bytes, &[], true).expect_err("incomplete descriptor list");
        assert_eq!(error.raw_os_error(), Some(libc::EIO));
    }
}

#[test]
fn strict_cleanup_reports_fcntl_failure_while_best_effort_ignores_it() {
    let bytes = entry(i32::MAX.to_string().as_bytes());
    let error = mark_proc_entries(&bytes, &[], true).expect_err("invalid descriptor must fail");
    assert_eq!(error.raw_os_error(), Some(libc::EBADF));
    assert!(mark_proc_entries(&bytes, &[], false).is_ok());
}

#[test]
fn cleanup_failure_reaches_parent_before_target_exec() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let marker = root.path().join("ran");
    let mut command = std::process::Command::new("/bin/sh");
    command
        .args(["-c", "printf ran > \"$1\"", "sh"])
        .arg(&marker);
    // SAFETY: The injected range/procfs failure uses only a syscall and a raw
    // OS error after fork; no allocation, logging, or locking occurs here.
    unsafe {
        command
            .pre_exec(|| cleanup_with_fallback(&[], |_, _| false, || mark_cloexec(i32::MAX, &[])));
    }
    let error = match command.spawn() {
        Err(error) => error,
        Ok(mut child) => {
            child.kill()?;
            child.wait()?;
            anyhow::bail!("target started after cleanup failed");
        }
    };
    assert_eq!(error.raw_os_error(), Some(libc::EBADF));
    assert!(!marker.exists());
    Ok(())
}
