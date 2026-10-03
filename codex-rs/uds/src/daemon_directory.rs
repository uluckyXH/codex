//! The host-local rendezvous root for privileged app-server RPC sockets.
//!
//! Every listener uses this root, which sandboxes hide even before a daemon
//! starts. It must not depend on HOME, TMPDIR, CODEX_HOME, or command settings.

use std::ffi::CString;
use std::fs;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;

/// Keeps the platform runtime chain alive while a listener or sandbox is prepared.
#[derive(Debug)]
pub struct DaemonSocketDirectoryGuard {
    path: PathBuf,
    runtime: Option<crate::ProtectedRuntimeDirectory>,
}

impl DaemonSocketDirectoryGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn revalidate(&self) -> io::Result<()> {
        if let Some(runtime) = &self.runtime {
            runtime.revalidate()?;
        }
        Ok(())
    }

    pub fn open_runtime_lock(&self, name: &std::ffi::OsStr) -> io::Result<File> {
        self.runtime
            .as_ref()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "runtime lock requires the OHOS fixed directory contract",
                )
            })?
            .open_lock_file(name)
    }
}

pub fn prepare_shared_daemon_socket_directory_guard() -> io::Result<DaemonSocketDirectoryGuard> {
    #[cfg(target_env = "ohos")]
    {
        let runtime =
            crate::prepare_ohos_runtime_directory(crate::OhosRuntimePurpose::ControlSockets)?;
        Ok(DaemonSocketDirectoryGuard {
            path: runtime.path().to_path_buf(),
            runtime: Some(runtime),
        })
    }
    #[cfg(not(target_env = "ohos"))]
    {
        let path = shared_daemon_socket_directory()?;
        prepare_directory(&path, unsafe { libc::geteuid() })?;
        Ok(DaemonSocketDirectoryGuard {
            path,
            runtime: None,
        })
    }
}

/// Returns the fixed executor-local directory that every sandbox must hide.
pub fn shared_daemon_socket_directory() -> io::Result<PathBuf> {
    #[cfg(target_env = "ohos")]
    {
        Ok(prepare_shared_daemon_socket_directory_guard()?.path)
    }
    #[cfg(not(target_env = "ohos"))]
    {
        let uid = unsafe { libc::geteuid() };
        // Preserve Linux's shared /tmp and the macOS /tmp -> /private/tmp alias.
        let root = fs::canonicalize("/tmp").map_err(|error| {
            directory_error("resolve system temporary root", Path::new("/tmp"), error)
        })?;
        Ok(root.join(format!("codex-daemon-{uid}")))
    }
}

/// Creates the reserved directory, rejecting symlinks and unsafe owners/modes.
pub fn prepare_shared_daemon_socket_directory() -> io::Result<PathBuf> {
    Ok(prepare_shared_daemon_socket_directory_guard()?.path)
}

#[cfg(any(not(target_env = "ohos"), test))]
fn prepare_directory(directory: &Path, uid: libc::uid_t) -> io::Result<()> {
    let parent = directory.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "app-server socket directory must have a parent",
        )
    })?;
    let name = directory.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "app-server socket directory must have a basename",
        )
    })?;
    let name = CString::new(name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "app-server socket directory contains NUL",
        )
    })?;
    let parent_file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(parent)
        .map_err(|error| directory_error("open reserved directory parent", parent, error))?;
    // mkdirat/openat use the same parent, including when another process creates
    // the reservation concurrently. Never follow a pre-existing leaf symlink.
    if unsafe { libc::mkdirat(parent_file.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(directory_error(
                "create reserved directory",
                directory,
                error,
            ));
        }
    }
    let fd = unsafe {
        libc::openat(
            parent_file.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(directory_error(
            "open reserved directory without following links",
            directory,
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: openat returned a fresh owned descriptor.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file
        .metadata()
        .map_err(|error| directory_error("inspect reserved directory", directory, error))?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o7777 != 0o700 {
        return Err(directory_error(
            "validate reserved directory",
            directory,
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "app-server socket directory must be a user-owned directory with mode 0700",
            ),
        ));
    }
    let current = fs::symlink_metadata(directory)
        .map_err(|error| directory_error("recheck reserved directory path", directory, error))?;
    if !current.is_dir() || current.dev() != metadata.dev() || current.ino() != metadata.ino() {
        return Err(directory_error(
            "recheck reserved directory path",
            directory,
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "app-server socket directory changed during preparation",
            ),
        ));
    }
    Ok(())
}

fn directory_error(operation: &str, path: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "app-server socket directory: {operation} at {}: {error}",
            path.display()
        ),
    )
}

#[cfg(test)]
#[path = "daemon_directory_tests.rs"]
mod tests;
