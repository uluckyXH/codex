//! Bounded observation of sandbox capability probes on Unix.
//! A successful probe requires a successful exit; I/O failures stay errors.

use std::ffi::OsString;
use std::io;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Output;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

const OUTPUT_LIMIT: usize = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Find an executable probe command instead of assuming a Unix filesystem
/// layout. OHOS installations may supply native utilities through PATH; their
/// availability and sandbox visibility still require device verification.
pub fn resolve_true_command() -> io::Result<PathBuf> {
    resolve_true_command_in(
        &[
            // OpenHarmony toybox installs a `true` alias in system/bin.
            // Still require an executable: product images can omit utilities.
            #[cfg(target_env = "ohos")]
            Path::new("/system/bin/true"),
            Path::new("/usr/bin/true"),
            Path::new("/bin/true"),
        ],
        std::env::var_os("PATH"),
        &std::env::current_dir()?,
    )
}

fn resolve_true_command_in(
    candidates: &[&Path],
    search_path: Option<OsString>,
    cwd: &Path,
) -> io::Result<PathBuf> {
    for candidate in candidates {
        if let Ok(path) = which::which_in(candidate, None::<OsString>, cwd) {
            return Ok(path);
        }
    }
    which::which_in("true", Some(search_path.unwrap_or_default()), cwd).map_err(|error| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "sandbox probe command 'true' is unavailable: no executable in the standard utility locations or PATH ({error}); namespace support could not be verified"
            ),
        )
    })
}

/// Run a probe in its own process group, bounding time and both output streams.
/// On observation failure the child group is killed and the direct child reaped.
pub fn run(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let mut child = ProbeChild {
        child,
        reaped: false,
    };
    let stdout = child.child.stdout.take();
    let stderr = child.child.stderr.take();
    collect_output(stdout, stderr, timeout, || {
        let status = child.child.try_wait().inspect_err(|error| {
            // An externally reaped child no longer reserves this PID. Do not
            // signal a possibly reused PID while reporting the observation error.
            if error.raw_os_error() == Some(libc::ECHILD) {
                child.reaped = true;
            }
        })?;
        child.reaped = status.is_some();
        Ok(status)
    })
}

/// Observe pipes and child status without blocking on pipe EOF from descendants.
/// The caller owns child cleanup on error, including timeout. Pipes become
/// nonblocking; each stream is limited to 64 KiB. This also supports fork/exec
/// launchers that must keep their existing descriptor-based executable handling.
pub fn collect_output<S, E>(
    mut stdout: Option<S>,
    mut stderr: Option<E>,
    timeout: Duration,
    mut try_wait: impl FnMut() -> io::Result<Option<ExitStatus>>,
) -> io::Result<Output>
where
    S: Read + AsRawFd,
    E: Read + AsRawFd,
{
    if let Some(pipe) = &stdout {
        set_nonblocking(pipe)?;
    }
    if let Some(pipe) = &stderr {
        set_nonblocking(pipe)?;
    }
    let deadline = Instant::now() + timeout;
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    loop {
        drain(&mut stdout, &mut stdout_bytes)?;
        drain(&mut stderr, &mut stderr_bytes)?;
        if let Some(status) = try_wait()? {
            // Collect bytes written immediately before the observed exit, but
            // do not wait for inherited pipes held by surviving descendants.
            drain(&mut stdout, &mut stdout_bytes)?;
            drain(&mut stderr, &mut stderr_bytes)?;
            return Ok(Output {
                status,
                stdout: stdout_bytes,
                stderr: stderr_bytes,
            });
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "sandbox probe timed out",
            ));
        }
        std::thread::sleep(POLL_INTERVAL.min(remaining));
    }
}

fn set_nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    // SAFETY: the caller owns this pipe throughout observation.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn drain<R: Read>(pipe: &mut Option<R>, output: &mut Vec<u8>) -> io::Result<()> {
    let Some(pipe) = pipe else {
        return Ok(());
    };
    let mut buffer = [0; 4096];
    // Limit reads per poll as well as total retained output. A continuously
    // writing child cannot prevent the caller from checking its deadline.
    for _ in 0..16 {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                if output.len() + count > OUTPUT_LIMIT {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "sandbox probe output exceeded 64 KiB",
                    ));
                }
                output.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

struct ProbeChild {
    child: Child,
    reaped: bool,
}

impl Drop for ProbeChild {
    fn drop(&mut self) {
        if !self.reaped {
            // SAFETY: this unreaped child owns the process-group ID. Kill also
            // reaches probe descendants that may keep a pipe open.
            unsafe {
                libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;
