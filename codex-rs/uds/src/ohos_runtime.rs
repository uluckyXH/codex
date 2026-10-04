//! Protected OHOS directories, independent of tool-controlled environments.
//! Platform startup initializes its own data tree beneath a verified files root.
//! An available path or Context alone is never a claim of device support.

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

#[path = "ohos_data.rs"]
mod data;
pub use data::OhosDataDirectories;
pub use data::OhosDirectorySource;
pub use data::OhosProcessIdentity;
pub use data::initialize_ohos_data_directories;
pub use data::ohos_platform_files_candidate;

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
/// Used only by explicit diagnostic profiles. The platform profile forbids an
/// override and initializes a verified platform files namespace instead.
pub fn ohos_runtime_base_contract() -> Option<&'static str> {
    option_env!("CODEX_OHOS_RUNTIME_BASE")
}

/// A deployment trust boundary selected by the builder, not by a child tool.
pub fn ohos_runtime_profile_contract() -> &'static str {
    option_env!("CODEX_OHOS_RUNTIME_PROFILE").unwrap_or("platform")
}

const HDC_DEBUG_BASE: &str = "/data/local/tmp/cdx";
const HDC_SHELL_UID: libc::uid_t = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeProfile {
    Strict,
    HdcDebug,
}

impl RuntimeProfile {
    fn from_contract(profile: &str, base: &Path, uid: libc::uid_t) -> io::Result<Self> {
        match profile {
            "strict" => Ok(Self::Strict),
            "platform" if base.as_os_str().is_empty() => Ok(Self::Strict),
            "hdc-debug" if base == Path::new(HDC_DEBUG_BASE) && uid == HDC_SHELL_UID => {
                Ok(Self::HdcDebug)
            }
            _ => Err(at_path(
                "profile-contract",
                base,
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "unknown runtime profile or mismatched fixed base/identity: platform uses a verified platform files root; hdc-debug requires /data/local/tmp/cdx and UID 2000; installation-specific UID profiles are not supported",
                ),
            )),
        }
    }

    fn validate_scope(self, base: &Path, uid: libc::uid_t) -> io::Result<()> {
        if self == Self::HdcDebug && (uid != HDC_SHELL_UID || !base.starts_with(HDC_DEBUG_BASE)) {
            return Err(at_path(
                "profile-scope",
                base,
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "hdc-debug may only open the fixed shell-owned private runtime subtree",
                ),
            ));
        }
        Ok(())
    }
}

fn compiled_runtime_profile(uid: libc::uid_t) -> io::Result<RuntimeProfile> {
    RuntimeProfile::from_contract(
        ohos_runtime_profile_contract(),
        Path::new(ohos_runtime_base_contract().unwrap_or("")),
        uid,
    )
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
    profile: RuntimeProfile,
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
        if unsafe { libc::geteuid() } != self.uid {
            return Err(at_path(
                "identity-changed",
                self.path(),
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "effective UID changed after directory validation",
                ),
            ));
        }
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
            validate_profile_metadata(
                &directory.path,
                &held,
                self.uid,
                directory.private,
                self.profile,
            )?;
            validate_profile_metadata(
                &directory.path,
                &named,
                self.uid,
                directory.private,
                self.profile,
            )?;
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
        self.create_subdirectory(name, false)
    }

    #[cfg(any(target_env = "ohos", test))]
    fn ensure_private_subdirectory(&self, name: &OsStr) -> io::Result<Self> {
        self.create_subdirectory(name, true)
    }

    fn create_subdirectory(&self, name: &OsStr, reuse: bool) -> io::Result<Self> {
        self.revalidate()?;
        let mut directories = self.clone_directories()?;
        create_private_child(&mut directories, name, self.uid, reuse)?;
        let child = Self {
            directories,
            uid: self.uid,
            profile: self.profile,
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
    if ohos_runtime_profile_contract() == "platform" {
        return initialize_ohos_data_directories()?.prepare_runtime_directory(purpose);
    }
    let base = ohos_runtime_base_contract().ok_or_else(|| io::Error::new(
        io::ErrorKind::Unsupported,
        "OHOS runtime-dir stage=contract: CODEX_OHOS_RUNTIME_BASE was not bound at build time; no verified platform base is available; HOME/Context/temp fallback is disabled",
    ))?;
    let uid = unsafe { libc::geteuid() };
    let profile = compiled_runtime_profile(uid)?;
    prepare_fixed_base_with_profile(Path::new(base), uid, purpose, profile)
}

/// Read-only verification for diagnostic callers. Does not create or change
/// the candidate, and does not bind it as the application's runtime contract.
pub fn validate_ohos_runtime_base(base: &Path) -> io::Result<ProtectedRuntimeDirectory> {
    let uid = unsafe { libc::geteuid() };
    open_base_with_profile(base, uid, compiled_runtime_profile(uid)?)
}

#[cfg(test)]
fn prepare_fixed_base(
    base: &Path,
    uid: libc::uid_t,
    purpose: OhosRuntimePurpose,
) -> io::Result<ProtectedRuntimeDirectory> {
    prepare_fixed_base_with_profile(base, uid, purpose, RuntimeProfile::Strict)
}

fn prepare_fixed_base_with_profile(
    base: &Path,
    uid: libc::uid_t,
    purpose: OhosRuntimePurpose,
    profile: RuntimeProfile,
) -> io::Result<ProtectedRuntimeDirectory> {
    let mut guard = open_base_with_profile(base, uid, profile)?;
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

#[cfg(test)]
fn open_base(base: &Path, uid: libc::uid_t) -> io::Result<ProtectedRuntimeDirectory> {
    open_base_with_profile(base, uid, RuntimeProfile::Strict)
}

fn open_base_with_profile(
    base: &Path,
    uid: libc::uid_t,
    profile: RuntimeProfile,
) -> io::Result<ProtectedRuntimeDirectory> {
    profile.validate_scope(base, uid)?;
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
        validate_profile_metadata(&path, &file.metadata()?, uid, false, profile)?;
        directories.push(Directory {
            path,
            file,
            private: false,
        });
    }
    let guard = ProtectedRuntimeDirectory {
        directories,
        uid,
        profile,
    };
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
    let mut guard = open_base_with_profile(parent, uid, compiled_runtime_profile(uid)?)?;
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
    validate_profile_metadata(path, metadata, uid, private, RuntimeProfile::Strict)
}

fn validate_profile_metadata(
    path: &Path,
    metadata: &Metadata,
    uid: libc::uid_t,
    private: bool,
    profile: RuntimeProfile,
) -> io::Result<()> {
    let mode = metadata.mode() & 0o7777;
    let safe = directory_profile_mode_is_safe(
        path,
        metadata.uid(),
        metadata.gid(),
        uid,
        mode,
        private,
        profile,
    );
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
                    "owner={} group={} euid={uid} mode={mode:04o} profile={profile:?}; require {}; no existing permissions were changed",
                    metadata.uid(),
                    metadata.gid(),
                    if private {
                        "current-user-owned directory with mode 0700"
                    } else if profile == RuntimeProfile::HdcDebug {
                        "the exact platform ancestor UID/GID/mode contract and a shell-owned 0700 runtime base"
                    } else {
                        "root/current-user-owned directory without group/other writes, or root-owned sticky directory"
                    }
                ),
            ),
        ));
    }
    Ok(())
}

fn directory_profile_mode_is_safe(
    path: &Path,
    owner: u32,
    group: u32,
    uid: u32,
    mode: u32,
    private: bool,
    profile: RuntimeProfile,
) -> bool {
    if profile == RuntimeProfile::HdcDebug {
        if uid != HDC_SHELL_UID {
            return false;
        }
        if !private {
            // OpenHarmony startup_init/services/etc/init.cfg creates these
            // exact ancestors. The explicit debugging deployment trusts the
            // OS system principal and the hdc shell group (including platform
            // diagnostic services), never arbitrary group-writable paths.
            // Production packages retain the Strict policy. This grants no
            // OS permission and does not claim command sandbox support.
            match path.to_str() {
                Some("/data") => return owner == 1000 && group == 1000 && mode == 0o771,
                Some("/data/local") => return owner == 0 && group == 0 && mode == 0o751,
                Some("/data/local/tmp") => {
                    return owner == HDC_SHELL_UID && group == HDC_SHELL_UID && mode == 0o771;
                }
                Some(HDC_DEBUG_BASE) => return owner == uid && mode == 0o700,
                _ => {}
            }
        }
    }
    directory_mode_is_safe(owner, uid, mode, private)
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
