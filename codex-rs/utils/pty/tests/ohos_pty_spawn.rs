//! Public-entry OHOS regression with no registered process-setup helper.
//! Failure injection is confined to a separate test process.

#![cfg(target_env = "ohos")]

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::process::Command;

use anyhow::Context;
use pretty_assertions::assert_eq;

const FIXTURE_ROOT: &str = "CODEX_TEST_OHOS_PTY_ROOT";

#[test]
fn public_pty_entry_rejects_cleanup_failure_without_helper() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    std::os::unix::fs::symlink("/bin/sh", root.path().join("shell"))?;
    // This integration executable never calls init_spawn_helper. The fixture
    // therefore exercises the public entry with an unavailable helper.
    let output = Command::new(std::env::current_exe()?)
        .args([
            "--ignored",
            "--exact",
            "public_pty_cleanup_failure_child",
            "--nocapture",
        ])
        .env(FIXTURE_ROOT, root.path())
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "PTY cleanup fixture failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("checked all four PTY launch forms"),
        "fixture did not exercise the public entry"
    );
    Ok(())
}

fn deny_cleanup_syscalls() -> io::Result<()> {
    let mut filter = [
        libc::sock_filter {
            code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as _,
            jt: 0,
            jf: 0,
            k: std::mem::offset_of!(libc::seccomp_data, nr) as _,
        },
        libc::sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as _,
            jt: 0,
            jf: 1,
            k: libc::SYS_close_range as _,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as _,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as _,
            jt: 0,
            jf: 1,
            k: libc::SYS_getdents64 as _,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as _,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as _,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ALLOW,
        },
    ];
    let program = libc::sock_fprog {
        len: filter.len() as _,
        filter: filter.as_mut_ptr(),
    };
    // SAFETY: The kernel copies this stack-owned filter. Only the isolated
    // fixture's calling thread and its future children receive the restriction.
    unsafe {
        if libc::prctl(
            libc::PR_SET_NO_NEW_PRIVS,
            1_usize,
            0_usize,
            0_usize,
            0_usize,
        ) == -1
            || libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER as usize,
                &raw const program,
                0_usize,
                0_usize,
            ) == -1
        {
            return Err(io::Error::last_os_error());
        }
        for syscall in [libc::SYS_close_range, libc::SYS_getdents64] {
            if libc::syscall(syscall, -1_isize, 0_usize, 0_usize) != -1
                || io::Error::last_os_error().raw_os_error() != Some(libc::EPERM)
            {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "isolated child of public_pty_entry_rejects_cleanup_failure_without_helper"]
fn public_pty_cleanup_failure_child() -> anyhow::Result<()> {
    let root = std::env::var(FIXTURE_ROOT).context("missing isolated fixture directory")?;
    let root = Path::new(&root);
    let absolute = root.join("shell").to_string_lossy().into_owned();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    deny_cleanup_syscalls()?;
    runtime.block_on(async {
        for (index, (program, arg0)) in [
            (absolute.as_str(), None),
            ("./shell", None),
            ("shell", None),
            (
                absolute.as_str(),
                Some("custom-argv0-not-a-program".to_owned()),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let marker = root.join(format!("ran-{index}"));
            let result = codex_utils_pty::spawn_pty_process(
                program,
                &[
                    "-c".to_owned(),
                    "printf ran > \"$1\"".to_owned(),
                    "sh".to_owned(),
                    marker.to_string_lossy().into_owned(),
                ],
                root,
                &HashMap::from([
                    ("PATH".to_owned(), root.to_string_lossy().into_owned()),
                    ("SHELL".to_owned(), "/bin/sh".to_owned()),
                ]),
                &arg0,
                codex_utils_pty::TerminalSize::default(),
                codex_utils_pty::ChildFds::Inherited(&[]),
            )
            .await;
            let error = match result {
                Err(error) => error,
                Ok(child) => {
                    child.session.terminate();
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), child.exit_rx)
                        .await;
                    anyhow::bail!("public PTY entry started {program} after cleanup failed");
                }
            };
            assert_eq!(
                error
                    .downcast_ref::<io::Error>()
                    .and_then(io::Error::raw_os_error),
                Some(libc::EPERM),
                "program={program}, arg0={arg0:?}, error={error}"
            );
            assert!(!marker.exists(), "target executed after failed cleanup");
        }
        println!("checked all four PTY launch forms");
        anyhow::Ok(())
    })
}
