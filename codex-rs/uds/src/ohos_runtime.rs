//! A build-bound OHOS runtime location, independent of tool environments.
//! The platform base is a deployment contract, not a claim of device support.
//! No candidate is selected by default and no per-process fallback is allowed.

use std::ffi::CString;
use std::ffi::OsStr;
use std::fs::File;
use std::fs::Metadata;
use std::io;
use std::os::fd::AsRawFd;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
#[cfg(not(target_env = "ohos"))]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OhosRuntimePurpose {
    Aliases,
    ControlSockets,
}

impl OhosRuntimePurpose {
    fn basename(self) -> &'static str {
        match self {
            Self::Aliases => "a",
            Self::ControlSockets => "s",
        }
    }
}

/// Set by the package builder, never by the environment of a running tool.
/// Changing this value requires a coordinated migration; old and new sandbox
/// instances cannot safely run concurrently with different socket roots.
pub fn ohos_runtime_base_contract() -> Option<&'static str> {
    option_env!("CODEX_OHOS_RUNTIME_BASE")
}

#[derive(Debug)]
struct Directory {
    path: PathBuf,
    file: File,
    private: bool,
}

/// Pins the validated directory chain. Descriptors are close-on-exec and must
/// never be included in the descriptor allowlist for an untrusted command.
#[derive(Debug)]
pub struct ProtectedRuntimeDirectory {
    directories: Vec<Directory>,
    uid: libc::uid_t,
}

impl ProtectedRuntimeDirectory {
    pub fn path(&self) -> &Path {
        &self.leaf().path
    }

    fn leaf(&self) -> &Directory {
        self.directories.last().expect("validated nonempty chain")
    }

    /// Reopen from / without following links and compare every pinned object.
    /// This detects replacement; it does not authorize a writable ancestor.
    pub fn revalidate(&self) -> io::Result<()> {
        let mut current = open_root()?;
        for (index, directory) in self.directories.iter().enumerate() {
            if index != 0 {
                current = open_directory_at(
                    &current,
                    directory.path.file_name().expect("non-root component"),
                    DIRECTORY_TRAVERSAL_ACCESS,
                )
                .map_err(|error| at_path("reopen", &directory.path, error))?;
            }
            let held = directory.file.metadata()?;
            let named = current.metadata()?;
            validate_metadata(&directory.path, &held, self.uid, directory.private)?;
            validate_metadata(&directory.path, &named, self.uid, directory.private)?;
            if held.dev() != named.dev() || held.ino() != named.ino() {
                return Err(at_path(
                    "identity-changed",
                    &directory.path,
                    io::Error::new(io::ErrorKind::PermissionDenied, "directory was replaced"),
                ));
            }
        }
        Ok(())
    }

    /// Create an exclusive private child, suitable for a randomized session.
    /// An existing object is never adopted, chmod'ed, or removed here.
    pub fn create_new_subdirectory(&self, name: &OsStr) -> io::Result<Self> {
        self.revalidate()?;
        let mut directories = self.clone_directories()?;
        create_private_child(&mut directories, name, self.uid, false)?;
        let child = Self {
            directories,
            uid: self.uid,
        };
        child.revalidate()?;
        Ok(child)
    }

    /// Open a lock beneath the pinned directory. No links, hard links, foreign
    /// owners, or unsafe existing modes are accepted. The caller acquires it.
    pub fn open_lock_file(&self, name: &OsStr) -> io::Result<File> {
        self.revalidate()?;
        let name_c = component_name(name)?;
        let path = self.path().join(name);
        let flags = libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
        let mut created = true;
        let mut fd = unsafe {
            libc::openat(
                self.as_raw_fd(),
                name_c.as_ptr(),
                flags | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
        };
        if fd < 0 && io::Error::last_os_error().kind() == io::ErrorKind::AlreadyExists {
            created = false;
            fd = unsafe { libc::openat(self.as_raw_fd(), name_c.as_ptr(), flags) };
        }
        if fd < 0 {
            return Err(at_path("open-lock", &path, io::Error::last_os_error()));
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.uid() != self.uid || metadata.nlink() != 1 {
            return Err(at_path(
                "validate-lock",
                &path,
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "lock must be an owned regular file with one link",
                ),
            ));
        }
        if created && metadata.mode() & 0o7777 != 0o600 {
            set_new_mode(&file, &path, 0o600)?;
        }
        if file.metadata()?.mode() & 0o7777 != 0o600 {
            return Err(at_path(
                "validate-lock",
                &path,
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "existing lock must have mode 0600",
                ),
            ));
        }
        self.validate_lock_file(name, &file)?;
        Ok(file)
    }

    /// Recheck a lock after acquiring it, rejecting unlinked/replaced files.
    pub fn validate_lock_file(&self, name: &OsStr, file: &File) -> io::Result<()> {
        self.revalidate()?;
        let held = file.metadata()?;
        let named = self.stat_child(name)?;
        if named.st_dev as u64 != held.dev()
            || named.st_ino as u64 != held.ino()
            || held.nlink() != 1
            || held.uid() != self.uid
            || !held.is_file()
            || held.mode() & 0o7777 != 0o600
        {
            return Err(at_path(
                "lock-identity",
                &self.path().join(name),
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "lock was replaced or changed",
                ),
            ));
        }
        Ok(())
    }

    pub(crate) fn require_private_leaf(&mut self) -> io::Result<()> {
        validate_metadata(self.path(), &self.leaf().file.metadata()?, self.uid, true)?;
        self.directories
            .last_mut()
            .expect("validated chain")
            .private = true;
        self.revalidate()
    }

    pub(crate) fn stat_child(&self, name: &OsStr) -> io::Result<libc::stat> {
        let name = component_name(name)?;
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                self.as_raw_fd(),
                name.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { metadata.assume_init() })
    }

    pub(crate) fn remove_child(&self, name: &OsStr) -> io::Result<()> {
        let name = component_name(name)?;
        if unsafe { libc::unlinkat(self.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn clone_directories(&self) -> io::Result<Vec<Directory>> {
        self.directories
            .iter()
            .map(|directory| {
                Ok(Directory {
                    path: directory.path.clone(),
                    file: directory.file.try_clone()?,
                    private: directory.private,
                })
            })
            .collect()
    }
}

impl AsRawFd for ProtectedRuntimeDirectory {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.leaf().file.as_raw_fd()
    }
}

pub fn prepare_ohos_runtime_directory(
    purpose: OhosRuntimePurpose,
) -> io::Result<ProtectedRuntimeDirectory> {
    let base = ohos_runtime_base_contract().ok_or_else(|| io::Error::new(
        io::ErrorKind::Unsupported,
        "OHOS runtime-dir stage=contract: CODEX_OHOS_RUNTIME_BASE was not bound at build time; no verified platform base is available; HOME/Context/temp fallback is disabled",
    ))?;
    prepare_fixed_base(Path::new(base), unsafe { libc::geteuid() }, purpose)
}

/// Read-only verification for diagnostic callers. Does not create or change
/// the candidate, and does not bind it as the application's runtime contract.
pub fn validate_ohos_runtime_base(base: &Path) -> io::Result<ProtectedRuntimeDirectory> {
    open_base(base, unsafe { libc::geteuid() })
}

fn prepare_fixed_base(
    base: &Path,
    uid: libc::uid_t,
    purpose: OhosRuntimePurpose,
) -> io::Result<ProtectedRuntimeDirectory> {
    let mut guard = open_base(base, uid)?;
    let user_directory = format!("c{uid:08x}");
    let path = base.join(&user_directory).join(purpose.basename());
    // Reject an overlong socket address before creating runtime objects.
    if purpose == OhosRuntimePurpose::ControlSockets {
        check_socket_budget(&path)?;
    }
    create_private_child(
        &mut guard.directories,
        OsStr::new(&user_directory),
        uid,
        true,
    )?;
    create_private_child(
        &mut guard.directories,
        OsStr::new(purpose.basename()),
        uid,
        true,
    )?;
    guard.revalidate()?;
    debug_assert_eq!(guard.path(), path);
    Ok(guard)
}

fn open_base(base: &Path, uid: libc::uid_t) -> io::Result<ProtectedRuntimeDirectory> {
    if !base.is_absolute()
        || base.parent().is_none()
        || base
            .as_os_str()
            .as_bytes()
            .split(|byte| *byte == b'/')
            .any(|part| part == b"." || part == b"..")
    {
        return Err(at_path(
            "contract",
            base,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "base must be an absolute non-root path without dot components",
            ),
        ));
    }
    let root = open_root()?;
    validate_metadata(Path::new("/"), &root.metadata()?, uid, false)?;
    let mut directories = vec![Directory {
        path: PathBuf::from("/"),
        file: root,
        private: false,
    }];
    for component in base.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        let parent = directories.last().expect("root is present");
        let path = parent.path.join(name);
        let file = open_directory_at(&parent.file, name, DIRECTORY_TRAVERSAL_ACCESS)
            .map_err(|error| at_path("open-ancestor", &path, error))?;
        validate_metadata(&path, &file.metadata()?, uid, false)?;
        directories.push(Directory {
            path,
            file,
            private: false,
        });
    }
    let guard = ProtectedRuntimeDirectory { directories, uid };
    guard.revalidate()?;
    Ok(guard)
}

/// Compatibility entry point for other OHOS private-directory callers.
/// Existing directories are validated, never repaired.
#[cfg(any(target_env = "ohos", test))]
pub(crate) fn prepare_private_directory(path: &Path) -> io::Result<ProtectedRuntimeDirectory> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "private directory must have a parent",
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "private directory must have a name",
        )
    })?;
    let uid = unsafe { libc::geteuid() };
    let mut guard = open_base(parent, uid)?;
    create_private_child(&mut guard.directories, name, uid, true)?;
    guard.revalidate()?;
    Ok(guard)
}

const DIRECTORY_FLAGS: libc::c_int = libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
#[cfg(target_env = "ohos")]
const DIRECTORY_TRAVERSAL_ACCESS: libc::c_int = libc::O_PATH;
#[cfg(not(target_env = "ohos"))]
const DIRECTORY_TRAVERSAL_ACCESS: libc::c_int = libc::O_RDONLY;

#[cfg(target_env = "ohos")]
fn open_root() -> io::Result<File> {
    // OHOS/musl includes O_PATH in O_ACCMODE. OpenOptions::custom_flags
    // masks access-mode bits, so use libc directly to retain O_PATH.
    let fd = unsafe { libc::open(c"/".as_ptr(), DIRECTORY_TRAVERSAL_ACCESS | DIRECTORY_FLAGS) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(not(target_env = "ohos"))]
fn open_root() -> io::Result<File> {
    File::options()
        .read(true)
        .custom_flags(DIRECTORY_FLAGS)
        .open("/")
}

pub(crate) fn component_name(name: &OsStr) -> io::Result<CString> {
    if name.is_empty() || name == "." || name == ".." || name.as_bytes().contains(&b'/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "runtime-dir child name must be one normal component",
        ));
    }
    CString::new(name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "runtime-dir child name contains NUL",
        )
    })
}

fn open_directory_at(parent: &File, name: &OsStr, access: libc::c_int) -> io::Result<File> {
    let name = component_name(name)?;
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), access | DIRECTORY_FLAGS) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn validate_metadata(
    path: &Path,
    metadata: &Metadata,
    uid: libc::uid_t,
    private: bool,
) -> io::Result<()> {
    let mode = metadata.mode() & 0o7777;
    let safe = directory_mode_is_safe(metadata.uid(), uid, mode, private);
    if !metadata.is_dir() || !safe {
        return Err(at_path(
            if private {
                "validate-private"
            } else {
                "validate-ancestor"
            },
            path,
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "owner={} euid={uid} mode={mode:04o}; require {}; no existing permissions were changed",
                    metadata.uid(),
                    if private {
                        "current-user-owned directory with mode 0700"
                    } else {
                        "root/current-user-owned directory without group/other writes, or root-owned sticky directory"
                    }
                ),
            ),
        ));
    }
    Ok(())
}

fn directory_mode_is_safe(owner: u32, uid: u32, mode: u32, private: bool) -> bool {
    if private {
        owner == uid && mode == 0o700
    } else {
        (owner == uid || owner == 0) && (mode & 0o022 == 0 || (owner == 0 && mode & 0o1000 != 0))
    }
}

fn create_private_child(
    directories: &mut Vec<Directory>,
    name: &OsStr,
    uid: libc::uid_t,
    reuse: bool,
) -> io::Result<()> {
    let name_c = component_name(name)?;
    let parent = directories.last().expect("validated parent");
    let path = parent.path.join(name);
    let created = unsafe { libc::mkdirat(parent.file.as_raw_fd(), name_c.as_ptr(), 0o700) } == 0;
    if !created {
        let error = io::Error::last_os_error();
        if !reuse || error.kind() != io::ErrorKind::AlreadyExists {
            return Err(at_path("mkdirat", &path, error));
        }
    }
    // A path-only FD is sufficient for ancestor traversal, fstat and *at
    // operations, but fchmod of a newly created leaf needs an ordinary FD.
    let file = open_directory_at(&parent.file, name, libc::O_RDONLY)
        .map_err(|error| at_path("open-private", &path, error))?;
    let metadata = file.metadata()?;
    if created && metadata.is_dir() && metadata.uid() == uid && metadata.mode() & 0o7777 != 0o700 {
        set_new_mode(&file, &path, 0o700)?;
    }
    validate_metadata(&path, &file.metadata()?, uid, true)?;
    directories.push(Directory {
        path,
        file,
        private: true,
    });
    Ok(())
}

fn set_new_mode(file: &File, path: &Path, mode: libc::mode_t) -> io::Result<()> {
    if unsafe { libc::fchmod(file.as_raw_fd(), mode) } != 0 {
        return Err(at_path("set-new-mode", path, io::Error::last_os_error()));
    }
    Ok(())
}

pub(crate) fn check_socket_budget(directory: &Path) -> io::Result<()> {
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if directory.as_os_str().as_bytes().len() + 1 + 64 + 1 > address.sun_path.len() {
        return Err(at_path(
            "socket-path-budget",
            directory,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "directory plus full 64-byte socket name and NUL exceeds sockaddr_un.sun_path",
            ),
        ));
    }
    Ok(())
}

fn at_path(stage: &str, path: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "OHOS runtime-dir stage={stage} path={}: {error}",
            path.display()
        ),
    )
}

#[cfg(test)]
#[path = "ohos_runtime_tests.rs"]
mod tests;
