//! Read-only helper entries installed by the platform HNP installer.
//!
//! Application processes cannot create symlinks in the tested HNP context. The
//! distribution instead installs four copies of one tiny signed launcher. Its
//! digest is bound at build time; no runtime variable selects helper contents.

use sha2::Digest;
use std::ffi::CString;
use std::fs::File;
use std::io;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

const INSTALLER_UID: u32 = 3060;
const MAX_LAUNCHER_BYTES: u64 = 1024 * 1024;

struct PinnedObject {
    path: PathBuf,
    file: File,
    directory: bool,
    launcher: bool,
}

pub(super) struct InstalledAliases {
    path: PathBuf,
    root: PathBuf,
    objects: Vec<PinnedObject>,
    expected_digest: [u8; 32],
    installer_uid: u32,
}

impl InstalledAliases {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    fn validate_process_executable(
        &self,
        executable: &Path,
        process_executable: &Path,
    ) -> io::Result<()> {
        let named = self
            .objects
            .iter()
            .find(|object| object.path == executable)
            .ok_or_else(|| denied("main executable was not pinned"))?
            .file
            .metadata()?;
        let running = std::fs::metadata(process_executable)?;
        if named.dev() != running.dev() || named.ino() != running.ino() {
            return Err(denied(
                "installed bin/codex does not identify this process executable",
            ));
        }
        Ok(())
    }

    fn revalidate(&self) -> io::Result<()> {
        for object in &self.objects {
            validate_metadata(
                &object.file,
                &object.path,
                object.directory,
                self.installer_uid,
            )?;
            let named = open_from_root(
                &self.root,
                &object.path,
                object.directory,
                self.installer_uid,
            )?;
            let held = object.file.metadata()?;
            let current = named.metadata()?;
            if held.dev() != current.dev() || held.ino() != current.ino() {
                return Err(denied("installed helper path was replaced"));
            }
            if object.launcher {
                validate_digest(&named, &self.expected_digest)?;
            }
        }
        Ok(())
    }
}

#[cfg(target_env = "ohos")]
pub(super) fn prepare(executable: &Path, aliases: &[&str]) -> io::Result<InstalledAliases> {
    validate_application_identity()?;
    if !executable.starts_with("/data/app") {
        return Err(denied(
            "HNP aliases require the private installed /data/app package",
        ));
    }
    let expected = option_env!("CODEX_HNP_ALIAS_SHA256")
        .ok_or_else(|| denied("HNP launcher digest was not bound at build time"))?;
    let guard =
        prepare_with_installer(Path::new("/"), executable, aliases, expected, INSTALLER_UID)?;
    // The named bin/codex must still identify the executable loaded by the OS.
    guard.validate_process_executable(executable, Path::new("/proc/self/exe"))?;
    Ok(guard)
}

#[cfg(target_env = "ohos")]
fn validate_application_identity() -> io::Result<()> {
    let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut groups = vec![0; count as usize];
    let actual = unsafe { libc::getgroups(count, groups.as_mut_ptr()) };
    if actual < 0 {
        return Err(io::Error::last_os_error());
    }
    groups.truncate(actual as usize);
    if application_identity_is_unprivileged(
        unsafe { libc::geteuid() },
        unsafe { libc::getegid() },
        &groups,
    ) && application_identity_is_unprivileged(
        unsafe { libc::getuid() },
        unsafe { libc::getgid() },
        &groups,
    ) {
        Ok(())
    } else {
        Err(denied(
            "HNP application must not run as or belong to the trusted platform installer identity",
        ))
    }
}

fn application_identity_is_unprivileged(uid: u32, gid: u32, groups: &[u32]) -> bool {
    uid >= 10_000 && gid != INSTALLER_UID && !groups.contains(&INSTALLER_UID)
}

fn prepare_with_installer(
    root: &Path,
    executable: &Path,
    aliases: &[&str],
    expected_digest: &str,
    installer_uid: u32,
) -> io::Result<InstalledAliases> {
    if executable.file_name() != Some(std::ffi::OsStr::new("codex")) {
        return Err(denied("HNP main executable must be named bin/codex"));
    }
    let bin = executable
        .parent()
        .ok_or_else(|| denied("missing HNP bin directory"))?;
    if bin.file_name() != Some(std::ffi::OsStr::new("bin")) {
        return Err(denied("HNP main executable must be inside bin"));
    }
    let package = bin
        .parent()
        .ok_or_else(|| denied("missing HNP package directory"))?;
    let path = package.join("codex-path");
    let expected_digest = parse_digest(expected_digest)?;
    let mut guard = InstalledAliases {
        path,
        root: root.to_path_buf(),
        objects: Vec::new(),
        expected_digest,
        installer_uid,
    };
    for target in [executable.to_path_buf(), guard.path.clone()] {
        pin_path(&mut guard, &target, target != executable, false)?;
    }
    for alias in aliases {
        if !matches!(
            *alias,
            "apply_patch" | "applypatch" | "codex-linux-sandbox" | "codex-execve-wrapper"
        ) {
            return Err(denied("unknown HNP helper name"));
        }
        let path = guard.path.join(alias);
        pin_path(&mut guard, &path, false, true)?;
    }
    guard.revalidate()?;
    Ok(guard)
}

fn parse_digest(value: &str) -> io::Result<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(denied(
            "HNP launcher digest must contain 64 hexadecimal digits",
        ));
    }
    let mut result = [0; 32];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| denied("invalid HNP launcher digest"))?;
    }
    Ok(result)
}

fn pin_path(
    guard: &mut InstalledAliases,
    path: &Path,
    directory: bool,
    launcher: bool,
) -> io::Result<()> {
    let mut chain = open_chain(&guard.root, path, directory, guard.installer_uid)?;
    if let Some(last) = chain.last_mut() {
        last.launcher = launcher;
        if launcher {
            validate_digest(&last.file, &guard.expected_digest)?;
        }
    }
    for object in chain {
        if let Some(previous) = guard
            .objects
            .iter()
            .find(|previous| previous.path == object.path)
        {
            let held = previous.file.metadata()?;
            let current = object.file.metadata()?;
            if held.dev() != current.dev() || held.ino() != current.ino() {
                return Err(denied("HNP ancestor changed while pinning aliases"));
            }
        } else {
            guard.objects.push(object);
        }
    }
    Ok(())
}

fn open_from_root(
    root: &Path,
    path: &Path,
    directory: bool,
    installer_uid: u32,
) -> io::Result<File> {
    open_chain(root, path, directory, installer_uid)?
        .pop()
        .map(|object| object.file)
        .ok_or_else(|| denied("HNP path has no root"))
}

fn open_chain(
    root: &Path,
    path: &Path,
    directory: bool,
    installer_uid: u32,
) -> io::Result<Vec<PinnedObject>> {
    if !path.is_absolute()
        || path
            .as_os_str()
            .as_bytes()
            .split(|byte| *byte == b'/')
            .any(|part| part == b"." || part == b"..")
    {
        return Err(denied("HNP path must be absolute without dot components"));
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| denied("HNP path escaped its root"))?;
    let current = open_component(None, root.as_os_str(), true)?;
    validate_metadata(&current, root, true, installer_uid)?;
    let mut chain = vec![PinnedObject {
        path: root.to_path_buf(),
        file: current,
        directory: true,
        launcher: false,
    }];
    let mut current_path = root.to_path_buf();
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err(denied("invalid HNP path component"));
        };
        let component_directory = index + 1 != components.len() || directory;
        let parent = chain.last().ok_or_else(|| denied("missing HNP parent"))?;
        let current = open_component(Some(&parent.file), name, component_directory)?;
        current_path.push(name);
        validate_metadata(&current, &current_path, component_directory, installer_uid)?;
        chain.push(PinnedObject {
            path: current_path.clone(),
            file: current,
            directory: component_directory,
            launcher: false,
        });
    }
    Ok(chain)
}

fn open_component(
    parent: Option<&File>,
    name: &std::ffi::OsStr,
    directory: bool,
) -> io::Result<File> {
    let name = CString::new(name.as_bytes()).map_err(|_| denied("NUL in HNP path"))?;
    #[cfg(target_env = "ohos")]
    let traversal = libc::O_PATH;
    #[cfg(not(target_env = "ohos"))]
    let traversal = libc::O_RDONLY;
    let access = if directory {
        traversal | libc::O_DIRECTORY
    } else {
        libc::O_RDONLY | libc::O_NONBLOCK
    };
    let flags = access | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let descriptor = unsafe {
        match parent {
            Some(parent) => libc::openat(parent.as_raw_fd(), name.as_ptr(), flags),
            None => libc::open(name.as_ptr(), flags),
        }
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn validate_metadata(
    file: &File,
    path: &Path,
    directory: bool,
    installer_uid: u32,
) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata_is_protected(
        path,
        metadata.uid(),
        metadata.gid(),
        metadata.mode(),
        directory,
        installer_uid,
    ) {
        return Err(denied(
            "HNP path must be root/installer-owned and immutable to the app; only installer 3060:3060/0775 directories under /data/app permit group write",
        ));
    }
    Ok(())
}

fn metadata_is_protected(
    path: &Path,
    uid: u32,
    gid: u32,
    mode: u32,
    directory: bool,
    installer_uid: u32,
) -> bool {
    let correct_type = if directory {
        mode & libc::S_IFMT as u32 == libc::S_IFDIR as u32
    } else {
        mode & libc::S_IFMT as u32 == libc::S_IFREG as u32 && mode & 0o001 != 0
    };
    // This exception trusts only the OS HNP installer group, never an app's
    // own writable group. Application membership is rejected before traversal.
    let installer_directory = directory
        && path.starts_with("/data/app")
        && uid == INSTALLER_UID
        && gid == INSTALLER_UID
        && installer_uid == INSTALLER_UID
        && mode & 0o7777 == 0o775;
    correct_type
        && (uid == 0 || uid == installer_uid)
        && (mode & 0o7022 == 0 || installer_directory)
}

fn validate_digest(file: &File, expected: &[u8; 32]) -> io::Result<()> {
    if file.metadata()?.len() > MAX_LAUNCHER_BYTES {
        return Err(denied("HNP launcher exceeds the small-helper size limit"));
    }
    let mut reader = file.try_clone()?;
    use std::io::Seek;
    reader.rewind()?;
    let mut digest = sha2::Sha256::new();
    let mut chunk = [0; 8192];
    let mut total = 0;
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_LAUNCHER_BYTES {
            return Err(denied("HNP launcher grew while hashing"));
        }
        digest.update(&chunk[..count]);
    }
    if digest.finalize().as_slice() != expected {
        return Err(denied(
            "installed HNP launcher digest does not match the build",
        ));
    }
    Ok(())
}

fn denied(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(test)]
#[path = "harmony_hnp_alias_dir_tests.rs"]
mod tests;
