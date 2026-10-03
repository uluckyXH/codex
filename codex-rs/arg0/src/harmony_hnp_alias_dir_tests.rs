use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;

const ALIASES: &[&str] = &[
    "apply_patch",
    "applypatch",
    "codex-linux-sandbox",
    "codex-execve-wrapper",
];
const LAUNCHER: &[u8] = b"signed launcher fixture";

struct Fixture {
    temporary: tempfile::TempDir,
    executable: PathBuf,
    aliases: PathBuf,
    digest: String,
    uid: u32,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let package = temporary.path().join("package.org/version");
        let executable = package.join("bin/codex");
        let aliases = package.join("codex-path");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::create_dir_all(&aliases).unwrap();
        for directory in [
            temporary.path().to_path_buf(),
            temporary.path().join("package.org"),
            package,
            executable.parent().unwrap().to_path_buf(),
            aliases.clone(),
        ] {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).unwrap();
        }
        write_executable(&executable, b"codex fixture");
        for alias in ALIASES {
            write_executable(&aliases.join(alias), LAUNCHER);
        }
        Self {
            temporary,
            executable,
            aliases,
            digest: format!("{:x}", sha2::Sha256::digest(LAUNCHER)),
            uid: unsafe { libc::geteuid() },
        }
    }

    fn prepare(&self) -> io::Result<InstalledAliases> {
        prepare_with_installer(
            self.temporary.path(),
            &self.executable,
            ALIASES,
            &self.digest,
            self.uid,
        )
    }
}

fn write_executable(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn installed_aliases_pin_all_directories_and_close_fds_on_exec() {
    let fixture = Fixture::new();
    let aliases = fixture.prepare().unwrap();
    assert_eq!(aliases.path(), fixture.aliases);
    assert_eq!(
        aliases
            .objects
            .iter()
            .filter(|object| object.directory)
            .count(),
        5
    );
    assert_eq!(
        aliases
            .objects
            .iter()
            .filter(|object| object.launcher)
            .count(),
        4
    );
    for object in &aliases.objects {
        assert_ne!(
            unsafe { libc::fcntl(object.file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
    }
    aliases.revalidate().unwrap();
    aliases
        .validate_process_executable(&fixture.executable, &fixture.executable)
        .unwrap();
    drop(aliases);
    assert!(fixture.executable.exists());
    assert!(fixture.aliases.join("apply_patch").exists());
}

#[test]
fn another_main_executable_identity_is_rejected() {
    let fixture = Fixture::new();
    let aliases = fixture.prepare().unwrap();
    let different = fixture.temporary.path().join("different");
    write_executable(&different, b"codex fixture");
    assert!(
        aliases
            .validate_process_executable(&fixture.executable, &different)
            .is_err()
    );
}

#[test]
fn wrong_layout_and_unknown_alias_are_rejected() {
    let fixture = Fixture::new();
    for executable in [
        fixture.executable.with_file_name("other"),
        fixture.aliases.join("codex"),
    ] {
        assert!(
            prepare_with_installer(
                fixture.temporary.path(),
                &executable,
                ALIASES,
                &fixture.digest,
                fixture.uid
            )
            .is_err()
        );
    }
    assert!(
        prepare_with_installer(
            fixture.temporary.path(),
            &fixture.executable,
            &["../bin/codex"],
            &fixture.digest,
            fixture.uid
        )
        .is_err()
    );
}

#[test]
fn malformed_digest_and_changed_launcher_bytes_are_rejected() {
    let fixture = Fixture::new();
    for digest in ["", "00", &"g".repeat(64)] {
        assert!(
            prepare_with_installer(
                fixture.temporary.path(),
                &fixture.executable,
                ALIASES,
                digest,
                fixture.uid
            )
            .is_err()
        );
    }
    let aliases = fixture.prepare().unwrap();
    write_executable(
        &fixture.aliases.join("apply_patch"),
        b"different launcher fixture",
    );
    assert!(aliases.revalidate().is_err());
    assert!(fixture.prepare().is_err());
}

#[test]
fn oversized_launcher_is_rejected_before_hashing() {
    let fixture = Fixture::new();
    File::options()
        .write(true)
        .open(fixture.aliases.join("apply_patch"))
        .unwrap()
        .set_len(MAX_LAUNCHER_BYTES + 1)
        .unwrap();
    assert!(fixture.prepare().is_err());
}

#[test]
fn foreign_owner_writable_or_special_modes_are_rejected() {
    let fixture = Fixture::new();
    assert!(
        prepare_with_installer(
            fixture.temporary.path(),
            &fixture.executable,
            ALIASES,
            &fixture.digest,
            fixture.uid + 1
        )
        .is_err()
    );
    for path in [
        fixture.executable.clone(),
        fixture.aliases.clone(),
        fixture.temporary.path().join("package.org"),
    ] {
        for mode in [0o775, 0o777, 0o4755, 0o2755, 0o1755] {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                fixture.prepare().is_err(),
                "accepted mode {mode:o} for {}",
                path.display()
            );
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::set_permissions(&fixture.executable, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(fixture.prepare().is_err());
}

#[test]
fn symlink_launchers_main_binary_and_directory_are_rejected() {
    for target in ["alias", "main", "directory"] {
        let fixture = Fixture::new();
        let path = match target {
            "alias" => fixture.aliases.join("apply_patch"),
            "main" => fixture.executable.clone(),
            _ => fixture.aliases.clone(),
        };
        let original = path.with_file_name("original");
        fs::rename(&path, &original).unwrap();
        symlink(&original, &path).unwrap();
        assert!(fixture.prepare().is_err(), "accepted symlink {target}");
    }
}

#[test]
fn replaced_directory_is_rejected_even_when_file_identity_is_preserved() {
    let fixture = Fixture::new();
    let aliases = fixture.prepare().unwrap();
    let original = fixture.aliases.with_file_name("old-path");
    fs::rename(&fixture.aliases, &original).unwrap();
    fs::create_dir(&fixture.aliases).unwrap();
    fs::set_permissions(&fixture.aliases, fs::Permissions::from_mode(0o755)).unwrap();
    for alias in ALIASES {
        fs::hard_link(original.join(alias), fixture.aliases.join(alias)).unwrap();
    }
    assert!(aliases.revalidate().is_err());
}

#[test]
fn same_digest_file_replacement_is_rejected_by_identity() {
    let fixture = Fixture::new();
    let aliases = fixture.prepare().unwrap();
    let path = fixture.aliases.join("apply_patch");
    fs::rename(&path, fixture.aliases.join("old-apply-patch")).unwrap();
    write_executable(&path, LAUNCHER);
    assert!(aliases.revalidate().is_err());
}

#[test]
fn dot_components_and_root_escape_are_rejected() {
    let fixture = Fixture::new();
    let dotted = fixture.aliases.join("../bin/codex");
    assert!(
        prepare_with_installer(
            fixture.temporary.path(),
            &dotted,
            ALIASES,
            &fixture.digest,
            fixture.uid
        )
        .is_err()
    );
    assert!(
        prepare_with_installer(
            &fixture.aliases,
            &fixture.executable,
            ALIASES,
            &fixture.digest,
            fixture.uid
        )
        .is_err()
    );
}

#[test]
fn installer_group_write_requires_the_exact_hnp_directory_contract() {
    let directory = libc::S_IFDIR as u32;
    let regular = libc::S_IFREG as u32;
    assert!(metadata_is_protected(
        Path::new("/data/app/package.org/version/bin"),
        3060,
        3060,
        directory | 0o775,
        true,
        3060
    ));
    for path in [
        "/data",
        "/data/application",
        "/data/storage/el2/base/files/r",
        "/tmp/hnp",
    ] {
        assert!(!metadata_is_protected(
            Path::new(path),
            3060,
            3060,
            directory | 0o775,
            true,
            3060
        ));
    }
    for (uid, gid, mode, kind) in [
        (3060, 1000, directory | 0o775, true),
        (0, 3060, directory | 0o775, true),
        (20020059, 3060, directory | 0o775, true),
        (3060, 3060, directory | 0o777, true),
        (3060, 3060, directory | 0o2775, true),
        (3060, 3060, regular | 0o775, false),
    ] {
        assert!(!metadata_is_protected(
            Path::new("/data/app/package.org/version/bin"),
            uid,
            gid,
            mode,
            kind,
            3060
        ));
    }
}

#[test]
fn installer_primary_or_supplementary_group_membership_is_rejected() {
    assert!(application_identity_is_unprivileged(
        20020059,
        20020059,
        &[1008]
    ));
    assert!(!application_identity_is_unprivileged(3060, 3060, &[]));
    assert!(!application_identity_is_unprivileged(20020059, 3060, &[]));
    assert!(!application_identity_is_unprivileged(
        20020059,
        20020059,
        &[1008, 3060]
    ));
}
