//! Per-process aliases below the shared, validated OHOS runtime contract.
//! Hold every validated directory FD. Never janitor old CODEX_HOME directories
//! or recursively remove names that may now identify somebody else's objects.

use codex_uds::ProtectedRuntimeDirectory;
use std::ffi::CString;
use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

struct LinkIdentity {
    name: CString,
    dev: libc::dev_t,
    ino: libc::ino_t,
}

struct DynamicAliases {
    root: ProtectedRuntimeDirectory,
    session: ProtectedRuntimeDirectory,
    name: CString,
    links: Vec<LinkIdentity>,
}

impl DynamicAliases {
    fn path(&self) -> &Path {
        self.session.path()
    }
}

enum AliasStorage {
    Dynamic(DynamicAliases),
    Installed {
        aliases: crate::harmony_hnp_alias_dir::InstalledAliases,
        _runtime: ProtectedRuntimeDirectory,
    },
}

pub(super) struct SessionAliases(AliasStorage);

impl SessionAliases {
    pub(super) fn path(&self) -> &Path {
        match &self.0 {
            AliasStorage::Dynamic(aliases) => aliases.path(),
            AliasStorage::Installed { aliases, .. } => aliases.path(),
        }
    }
}

#[cfg(target_env = "ohos")]
pub(super) fn prepare(executable: &Path, aliases: &[&str]) -> io::Result<SessionAliases> {
    let root = codex_uds::prepare_ohos_runtime_directory(codex_uds::OhosRuntimePurpose::Aliases)?;
    if uses_installed_aliases(executable) {
        let aliases = crate::harmony_hnp_alias_dir::prepare(executable, aliases)?;
        root.revalidate()?;
        return Ok(SessionAliases(AliasStorage::Installed {
            aliases,
            _runtime: root,
        }));
    }
    prepare_in(root, executable, aliases)
        .map(|aliases| SessionAliases(AliasStorage::Dynamic(aliases)))
}

fn uses_installed_aliases(executable: &Path) -> bool {
    // A bound digest describes installed helpers; it does not force a normal
    // terminal installation into the HNP namespace. The HNP branch still
    // requires that digest and validates installer ownership and file identity.
    executable.starts_with("/data/app")
}

fn prepare_in(
    root: ProtectedRuntimeDirectory,
    executable: &Path,
    aliases: &[&str],
) -> io::Result<DynamicAliases> {
    let target = c_string(executable.as_os_str())?;
    let mut random = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut random)?;
    let random = u128::from_ne_bytes(random);
    let name = format!("p{}-{random:032x}", std::process::id());
    let session = root.create_new_subdirectory(OsStr::new(&name))?;
    let mut guard = DynamicAliases {
        root,
        session,
        name: c_string(OsStr::new(&name))?,
        links: Vec::new(),
    };
    for alias in aliases {
        if alias.is_empty() || alias.contains('/') || matches!(*alias, "." | "..") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid alias component",
            ));
        }
        let name = c_string(OsStr::new(alias))?;
        guard.session.revalidate()?;
        if unsafe { libc::symlinkat(target.as_ptr(), guard.session.as_raw_fd(), name.as_ptr()) }
            != 0
        {
            return Err(io::Error::last_os_error());
        }
        let metadata = stat_at(&guard.session, &name)?;
        if metadata.st_mode & libc::S_IFMT != libc::S_IFLNK {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "created alias changed type",
            ));
        }
        guard.links.push(LinkIdentity {
            name,
            dev: metadata.st_dev,
            ino: metadata.st_ino,
        });
    }
    guard.session.revalidate()?;
    Ok(guard)
}

fn c_string(value: &OsStr) -> io::Result<CString> {
    CString::new(value.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

fn stat_at(directory: &ProtectedRuntimeDirectory, name: &CString) -> io::Result<libc::stat> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
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

impl Drop for DynamicAliases {
    fn drop(&mut self) {
        // A changed ancestor invalidates cleanup through the original pathname.
        // Preserve both trees rather than recursively following the new name.
        if self.root.revalidate().is_err() || self.session.revalidate().is_err() {
            return;
        }
        for link in &self.links {
            let Ok(current) = stat_at(&self.session, &link.name) else {
                continue;
            };
            if current.st_dev == link.dev
                && current.st_ino == link.ino
                && current.st_mode & libc::S_IFMT == libc::S_IFLNK
            {
                unsafe {
                    libc::unlinkat(self.session.as_raw_fd(), link.name.as_ptr(), 0);
                }
            }
        }
        // revalidate compares the root's child name with the pinned session FD.
        // AT_REMOVEDIR refuses unexpected content; no recursive cleanup occurs.
        if self.session.revalidate().is_ok() {
            unsafe {
                libc::unlinkat(
                    self.root.as_raw_fd(),
                    self.name.as_ptr(),
                    libc::AT_REMOVEDIR,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        #[cfg(target_os = "macos")]
        let temporary = tempfile::tempdir_in("/private/tmp").unwrap();
        #[cfg(not(target_os = "macos"))]
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().canonicalize().unwrap().join("aliases");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        (temporary, path)
    }

    #[test]
    fn alias_deployment_follows_the_process_installation_not_the_bound_digest() {
        assert!(uses_installed_aliases(Path::new(
            "/data/app/package/bin/codex"
        )));
        for path in [
            "/data/app-other/bin/codex",
            "/system/bin/codex",
            "/storage/Users/codex/bin/codex",
        ] {
            assert!(!uses_installed_aliases(Path::new(path)), "{path}");
        }
    }

    fn aliases(path: &Path) -> DynamicAliases {
        let root = codex_uds::validate_ohos_runtime_base(path).unwrap();
        prepare_in(
            root,
            Path::new("/usr/bin/true"),
            &["apply_patch", "applypatch"],
        )
        .unwrap()
    }

    #[test]
    fn aliases_are_executable_and_only_session_objects_are_removed() {
        let (_temporary, path) = fixture();
        fs::write(path.join("existing"), "keep").unwrap();
        let aliases = aliases(&path);
        let session = aliases.path().to_owned();
        assert_eq!(
            fs::read_link(session.join("apply_patch")).unwrap(),
            Path::new("/usr/bin/true")
        );
        assert!(
            std::process::Command::new(session.join("apply_patch"))
                .status()
                .unwrap()
                .success()
        );
        assert!(!session.join(".lock").exists());
        assert_ne!(
            unsafe { libc::fcntl(aliases.session.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        drop(aliases);
        assert!(!session.exists());
        assert_eq!(fs::read_to_string(path.join("existing")).unwrap(), "keep");
    }

    #[test]
    fn replaced_alias_and_unknown_content_are_preserved() {
        let (_temporary, path) = fixture();
        let aliases = aliases(&path);
        let session = aliases.path().to_owned();
        fs::remove_file(session.join("apply_patch")).unwrap();
        fs::write(session.join("apply_patch"), "replacement").unwrap();
        fs::write(session.join("unrelated"), "keep").unwrap();
        drop(aliases);
        assert_eq!(
            fs::read_to_string(session.join("apply_patch")).unwrap(),
            "replacement"
        );
        assert_eq!(
            fs::read_to_string(session.join("unrelated")).unwrap(),
            "keep"
        );
        assert!(!session.join("applypatch").exists());
    }

    #[test]
    fn replaced_ancestor_prevents_cleanup_of_both_trees() {
        let (_temporary, path) = fixture();
        let aliases = aliases(&path);
        let name = aliases.path().file_name().unwrap().to_owned();
        let original = path.with_file_name("original");
        fs::rename(&path, &original).unwrap();
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(path.join(&name)).unwrap();
        fs::write(path.join(&name).join("keep"), "replacement").unwrap();
        drop(aliases);
        assert!(original.join(&name).join("apply_patch").exists());
        assert_eq!(
            fs::read_to_string(path.join(name).join("keep")).unwrap(),
            "replacement"
        );
    }

    #[test]
    fn changed_parent_is_rejected_before_creating_a_session() {
        let (_temporary, path) = fixture();
        let root = codex_uds::validate_ohos_runtime_base(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o2771)).unwrap();
        assert!(prepare_in(root, Path::new("/usr/bin/true"), &["apply_patch"]).is_err());
        assert_eq!(fs::read_dir(path).unwrap().count(), 0);
    }
}
