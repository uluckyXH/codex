//! Linux descriptor cleanup after fork, using only stack storage and syscalls.
//! Stdio and explicitly preserved FDs remain usable. CLOEXEC keeps Rust's
//! spawn-error channel alive until exec succeeds. OHOS requires complete cleanup;
//! other Linux targets retain their existing best-effort launch behavior.

use std::ffi::CStr;
use std::io;
use std::os::fd::RawFd;

#[cfg(target_os = "linux")]
pub(crate) fn close_inherited_fds_except(preserved_fds: &[RawFd]) -> io::Result<()> {
    let strict = cfg!(target_env = "ohos");
    let result = cleanup_with_fallback(
        preserved_fds,
        |first, last| {
            // SAFETY: close_range only changes this child's descriptor flags.
            unsafe {
                libc::syscall(
                    libc::SYS_close_range,
                    first,
                    last,
                    libc::CLOSE_RANGE_CLOEXEC,
                ) == 0
            }
        },
        || close_from_proc(preserved_fds, strict),
    );
    if strict { result } else { Ok(()) }
}

fn cleanup_with_fallback(
    preserved_fds: &[RawFd],
    mut mark_range: impl FnMut(u32, u32) -> bool,
    fallback: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    // Mark rather than close: std::process still needs its CLOEXEC error
    // pipe if exec fails. Do not alter flags on explicitly preserved FDs.
    let mut first = 3_u32;
    loop {
        // The keep-list need not be sorted. Walking its gaps leaves each kept
        // FD's flags untouched, without allocating a sorted copy after fork.
        let next = preserved_fds
            .iter()
            .copied()
            .filter_map(|fd| u32::try_from(fd).ok())
            .filter(|&fd| fd >= first)
            .min();
        let last = next.map_or(u32::MAX, |fd| fd - 1);
        if first <= last && !mark_range(first, last) {
            return fallback();
        }
        match next {
            Some(fd) => first = fd + 1,
            None => return Ok(()),
        }
    }
}

fn mark_cloexec(fd: RawFd, preserved_fds: &[RawFd]) -> io::Result<()> {
    if fd <= libc::STDERR_FILENO || preserved_fds.contains(&fd) {
        return Ok(());
    }
    // SAFETY: fcntl operates on the child's descriptors without allocating.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if flags & libc::FD_CLOEXEC == 0 {
        // SAFETY: only adds CLOEXEC; existing flags and the descriptor remain intact.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn close_from_proc(preserved_fds: &[RawFd], strict: bool) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::fd::FromRawFd;
    use std::os::fd::OwnedFd;

    // Open after fork: a proc directory opened in the parent would enumerate
    // the parent's changing descriptor table instead of this child's snapshot.
    // SAFETY: the path is NUL-terminated and open does not allocate.
    let raw = unsafe {
        libc::open(
            c"/proc/self/fd".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if raw == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: open returned a new owned descriptor; dropping it only calls close.
    let directory = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut buffer = [0_u8; 4096];
    loop {
        // SAFETY: getdents64 writes at most buffer.len() bytes into stack storage.
        let count = unsafe {
            libc::syscall(
                libc::SYS_getdents64,
                directory.as_raw_fd(),
                buffer.as_mut_ptr(),
                buffer.len(),
            )
        };
        if count == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(error);
        }
        if count == 0 {
            return Ok(());
        }
        let entries = buffer
            .get(..count as usize)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EIO))?;
        mark_proc_entries(entries, preserved_fds, strict)?;
    }
}

fn mark_proc_entries(mut entries: &[u8], preserved_fds: &[RawFd], strict: bool) -> io::Result<()> {
    // Linux getdents64 records have two 64-bit fields, a u16 record length,
    // one type byte, then the NUL-terminated name (independent of libc ABI).
    while !entries.is_empty() {
        if entries.len() < 20 {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        let length = u16::from_ne_bytes([entries[16], entries[17]]) as usize;
        if length < 20 || length > entries.len() {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        let name = CStr::from_bytes_until_nul(&entries[19..length]);
        if let Ok(name) = &name
            && matches!(name.to_bytes(), b"." | b"..")
        {
            entries = &entries[length..];
            continue;
        }
        let fd = name
            .ok()
            .and_then(|name| name.to_str().ok())
            .and_then(|name| name.parse::<RawFd>().ok());
        if let Some(fd) = fd.filter(|fd| !strict || *fd >= 0) {
            let result = mark_cloexec(fd, preserved_fds);
            if strict {
                result?;
            }
        } else if strict {
            // Raw OS errors do not allocate, unlike formatted diagnostics.
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        entries = &entries[length..];
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
#[path = "linux_fds_tests.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "fd_cleanup_tests.rs"]
mod cleanup_tests;
