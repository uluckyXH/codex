//! OHOS may report the user's entire home as the OS temporary directory.
//! Keep helper aliases in an owned CODEX_HOME/tmp/arg0 directory, without
//! treating a validated home as a shared temporary root or changing HOME's mode.
//! Existing directories are checked, never repaired by following a symlink.

use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;

pub(super) fn prepare(
    codex_home: &Path,
    system_temp: &Path,
    user_home: Option<&Path>,
    uid: u32,
) -> io::Result<PathBuf> {
    let resolved_config = resolve_location(codex_home)?;
    let resolved_temp = resolve_location(system_temp)?;
    if resolved_config.starts_with(&resolved_temp) {
        let owned_home_is_temp = user_home
            .map(resolve_location)
            .transpose()?
            .filter(|home| home == &resolved_temp && home != &resolved_config)
            .map(|home| validate_directory(&home, uid, false))
            .transpose()?
            .is_some();
        if !owned_home_is_temp {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "alias-root: refusing CODEX_HOME {} under temporary root {}; only a non-writable-by-others, current-user-owned HOME may serve as the OHOS temporary root",
                    resolved_config.display(),
                    resolved_temp.display()
                ),
            ));
        }
    }

    // These directories are not shared daemon paths. The final alias root is
    // private even when an existing CODEX_HOME or tmp parent is readable.
    prepare_directory(codex_home, uid, false, true)?;
    let temporary = codex_home.join("tmp");
    prepare_directory(&temporary, uid, false, false)?;
    let aliases = temporary.join("arg0");
    prepare_directory(&aliases, uid, true, false)?;
    Ok(aliases)
}

/// Resolve existing ancestors as well, so a path alias cannot bypass the
/// temporary-root check when CODEX_HOME has not yet been created.
fn resolve_location(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "alias-root: an absolute path without '..' is required",
        ));
    }
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or(error)?;
            let name = path.file_name().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "alias-root: missing directory name",
                )
            })?;
            Ok(resolve_location(parent)?.join(name))
        }
        Err(error) => Err(at_path("resolve", path, error)),
    }
}

fn prepare_directory(path: &Path, uid: u32, private: bool, recursive: bool) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match fs::DirBuilder::new()
                .recursive(recursive)
                .mode(0o700)
                .create(path)
            {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(at_path("create", path, error)),
            }
        }
        Err(error) => return Err(at_path("inspect", path, error)),
    }
    validate_directory(path, uid, private)
}

fn validate_directory(path: &Path, uid: u32, private: bool) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| at_path("inspect", path, error))?;
    let mode = metadata.mode() & 0o7777;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid
        || mode & 0o022 != 0
        || (private && mode != 0o700)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "alias-root: refusing unsafe directory {} (owner {}, expected {}; mode {mode:04o}; require {}); no permissions were changed",
                path.display(),
                metadata.uid(),
                uid,
                if private {
                    "a non-symlink directory with mode 0700"
                } else {
                    "a non-symlink directory not writable by group or others"
                }
            ),
        ));
    }
    Ok(())
}

fn at_path(operation: &str, path: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("alias-root: {operation} {}: {error}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::fs::symlink;

    fn fixture() -> (tempfile::TempDir, PathBuf, u32) {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        fs::create_dir(&home).unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        let uid = fs::metadata(&home).unwrap().uid();
        (root, home, uid)
    }

    #[test]
    fn home_as_temp_creates_private_aliases_without_changing_home() {
        let (_root, home, uid) = fixture();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o755)).unwrap();
        let aliases = prepare(&home.join(".codex"), &home, Some(&home), uid).unwrap();
        assert_eq!(aliases, home.join(".codex/tmp/arg0"));
        assert_eq!(fs::metadata(aliases).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(home).unwrap().mode() & 0o777, 0o755);
    }

    #[test]
    fn separate_temp_keeps_configuration_out_of_temp() {
        let (_root, home, uid) = fixture();
        let temp = home.join("temporary");
        fs::create_dir(&temp).unwrap();
        prepare(&home.join("custom-config"), &temp, Some(&home), uid).unwrap();
        assert!(prepare(&temp.join("config"), &temp, Some(&home), uid).is_err());
        assert!(!temp.join("config").exists());
    }

    #[test]
    fn temporary_root_exception_requires_matching_owned_home() {
        let (root, home, uid) = fixture();
        for alternative in [None, Some(root.path())] {
            assert!(prepare(&home.join(".codex"), &home, alternative, uid).is_err());
        }
        assert!(prepare(&home.join(".codex"), &home, Some(&home), uid + 1).is_err());
        fs::set_permissions(&home, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(prepare(&home.join(".codex"), &home, Some(&home), uid).is_err());
        assert!(!home.join(".codex").exists());
    }

    #[test]
    fn directory_symlink_cannot_redirect_creation_or_chmod() {
        let (_root, home, uid) = fixture();
        let config = home.join(".codex");
        fs::create_dir(&config).unwrap();
        let outside = home.join("unrelated");
        fs::create_dir(&outside).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&outside, config.join("tmp")).unwrap();
        assert!(prepare(&config, &home, Some(&home), uid).is_err());
        assert!(!outside.join("arg0").exists());
        assert_eq!(fs::metadata(outside).unwrap().mode() & 0o777, 0o755);
    }

    #[test]
    fn canonical_home_alias_is_recognized_but_config_alias_into_temp_is_rejected() {
        let (root, home, uid) = fixture();
        let home_alias = root.path().join("home-alias");
        symlink(&home, &home_alias).unwrap();
        prepare(&home.join(".codex"), &home_alias, Some(&home), uid).unwrap();
        let temp = home.join("temporary");
        fs::create_dir(&temp).unwrap();
        let config_alias = home.join("config-alias");
        symlink(&temp, &config_alias).unwrap();
        assert!(prepare(&config_alias.join("config"), &temp, Some(&home), uid).is_err());
    }

    #[test]
    fn unsafe_existing_alias_directory_is_not_repaired() {
        let (_root, home, uid) = fixture();
        let config = home.join(".codex");
        let aliases = prepare(&config, &home, Some(&home), uid).unwrap();
        for mode in [0o755, 0o777] {
            fs::set_permissions(&aliases, fs::Permissions::from_mode(mode)).unwrap();
            assert!(prepare(&config, &home, Some(&home), uid).is_err());
            assert_eq!(fs::metadata(&aliases).unwrap().mode() & 0o777, mode);
        }
        fs::remove_dir(&aliases).unwrap();
        fs::write(&aliases, "keep").unwrap();
        assert!(prepare(&config, &home, Some(&home), uid).is_err());
        assert_eq!(fs::read_to_string(aliases).unwrap(), "keep");
    }
}
