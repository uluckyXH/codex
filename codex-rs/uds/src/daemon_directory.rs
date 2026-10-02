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

#[cfg(any(target_env = "ohos", test))]
const OHOS_CURRENT_USER_HOME: &str = "/storage/Users/currentUser";

/// Returns the fixed executor-local directory that every sandbox must hide.
pub fn shared_daemon_socket_directory() -> io::Result<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    #[cfg(target_env = "ohos")]
    {
        // PC /tmp may be read-only, and the PC user need not have a passwd
        // entry. Prefer the fixed PC system entry, validating its real UID and
        // path. Never use environment forwarded to a tool. A short basename
        // leaves room for the transport's full SHA256 socket name.
        let home = ohos_home(Path::new(OHOS_CURRENT_USER_HOME), uid, account_home)?;
        let directory = home.join(".codex-uds");
        check_socket_path_length(&directory)?;
        Ok(directory)
    }
    #[cfg(not(target_env = "ohos"))]
    {
        // Preserve Linux's shared /tmp and the macOS /tmp -> /private/tmp alias.
        let root = fs::canonicalize("/tmp").map_err(|error| {
            directory_error("resolve system temporary root", Path::new("/tmp"), error)
        })?;
        Ok(root.join(format!("codex-daemon-{uid}")))
    }
}

#[cfg(any(target_env = "ohos", test))]
fn ohos_home(
    platform_home: &Path,
    uid: libc::uid_t,
    query_account: impl FnOnce(libc::uid_t) -> io::Result<PathBuf>,
) -> io::Result<PathBuf> {
    match fs::symlink_metadata(platform_home) {
        // An existing but invalid entry must fail closed. In particular, a
        // dangling symlink must not silently select a different socket root.
        Ok(_) => trusted_account_home(platform_home, uid),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            query_account(uid).and_then(|home| trusted_account_home(&home, uid))
        }
        Err(error) => Err(directory_error(
            "inspect fixed OHOS user home",
            platform_home,
            error,
        )),
    }
}

/// Creates the reserved directory, rejecting symlinks and unsafe owners/modes.
pub fn prepare_shared_daemon_socket_directory() -> io::Result<PathBuf> {
    let directory = shared_daemon_socket_directory()?;
    prepare_directory(&directory, unsafe { libc::geteuid() })?;
    Ok(directory)
}

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

#[cfg(any(target_env = "ohos", test))]
fn account_home(uid: libc::uid_t) -> io::Result<PathBuf> {
    use std::ffi::CStr;
    use std::ffi::OsStr;
    let mut buffer = vec![0_u8; 1024];
    loop {
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // SAFETY: buffers are writable and live until all returned fields have
        // been copied. getpwuid_r does not use process-global passwd storage.
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                entry.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && buffer.len() < 1024 * 1024 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "resolve OHOS app-server socket account for uid {uid}: {}",
                    io::Error::from_raw_os_error(status)
                ),
            ));
        }
        if result.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "resolve OHOS app-server socket account: uid {uid} has no passwd entry; refusing environment-based fallback"
                ),
            ));
        }
        let entry = unsafe { entry.assume_init() };
        if entry.pw_uid != uid || entry.pw_dir.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "OHOS app-server socket account returned an invalid uid or home",
            ));
        }
        let path = unsafe { CStr::from_ptr(entry.pw_dir) };
        return Ok(PathBuf::from(OsStr::from_bytes(path.to_bytes())));
    }
}

#[cfg(any(target_env = "ohos", test))]
fn canonical_account_home(home: &Path, uid: libc::uid_t) -> io::Result<PathBuf> {
    if !home.is_absolute() || home.parent().is_none() {
        return Err(directory_error(
            "validate account home",
            home,
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "account home must be an absolute non-root directory",
            ),
        ));
    }
    let resolved = fs::canonicalize(home)
        .map_err(|error| directory_error("resolve account home", home, error))?;
    let metadata = fs::symlink_metadata(&resolved)
        .map_err(|error| directory_error("inspect account home", &resolved, error))?;
    if resolved.parent().is_none()
        || !metadata.is_dir()
        || metadata.uid() != uid
        || metadata.mode() & 0o022 != 0
    {
        return Err(directory_error(
            "validate account home",
            &resolved,
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "account home must be user-owned and not writable by group or others",
            ),
        ));
    }
    Ok(resolved)
}

#[cfg(any(target_env = "ohos", test))]
fn trusted_account_home(home: &Path, uid: libc::uid_t) -> io::Result<PathBuf> {
    let resolved = canonical_account_home(home, uid)?;
    // Verify both the account spelling and the canonical location: an unsafe
    // alias parent must not redirect different processes to different roots.
    for path in [home, resolved.as_path()] {
        for ancestor in path.ancestors().skip(1) {
            let metadata = fs::metadata(ancestor).map_err(|error| {
                directory_error("inspect account home ancestor", ancestor, error)
            })?;
            let trusted_owner = metadata.uid() == 0 || metadata.uid() == uid;
            let sticky_root = metadata.uid() == 0 && metadata.mode() & 0o1000 != 0;
            if !metadata.is_dir()
                || !trusted_owner
                || (metadata.mode() & 0o022 != 0 && !sticky_root)
            {
                return Err(directory_error(
                    "validate account home ancestor",
                    ancestor,
                    io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "account home has an unsafe writable ancestor",
                    ),
                ));
            }
        }
    }
    Ok(resolved)
}

#[cfg(any(target_env = "ohos", test))]
fn check_socket_path_length(directory: &Path) -> io::Result<()> {
    // app-server-transport uses a full SHA256 hexadecimal basename (64 bytes).
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if directory.as_os_str().as_bytes().len() + 1 + 64 >= address.sun_path.len() {
        return Err(directory_error(
            "validate socket path length",
            directory,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "reserved directory plus a 64-byte socket name exceeds the {}-byte UNIX socket limit",
                    address.sun_path.len() - 1
                ),
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
