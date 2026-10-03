use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;

fn fixture() -> (tempfile::TempDir, PathBuf, u32) {
    // A full ancestor check cannot accept a group-writable external volume.
    #[cfg(target_os = "macos")]
    let temporary = tempfile::Builder::new()
        .prefix("cr")
        .tempdir_in("/private/tmp")
        .unwrap();
    #[cfg(not(target_os = "macos"))]
    let temporary = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(temporary.path()).unwrap();
    fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
    let uid = unsafe { libc::geteuid() };
    (temporary, base, uid)
}

#[test]
fn aliases_use_owned_fixed_subdirectories_without_changing_base() {
    let (_temporary, base, uid) = fixture();
    fs::set_permissions(&base, fs::Permissions::from_mode(0o755)).unwrap();
    let guard = prepare_fixed_base(&base, uid, OhosRuntimePurpose::Aliases).unwrap();
    assert_eq!(guard.path(), base.join(format!("c{uid:08x}/a")));
    assert_eq!(fs::metadata(guard.path()).unwrap().mode() & 0o7777, 0o700);
    assert_eq!(fs::metadata(&base).unwrap().mode() & 0o7777, 0o755);
    assert_eq!(guard.leaf().file.metadata().unwrap().uid(), uid);
    assert_ne!(
        unsafe { libc::fcntl(guard.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    let session = guard
        .create_new_subdirectory(OsStr::new("session-one"))
        .unwrap();
    assert!(
        session
            .create_new_subdirectory(OsStr::new("../outside"))
            .is_err()
    );
    assert!(
        guard
            .create_new_subdirectory(OsStr::new("session-one"))
            .is_err()
    );
    session.revalidate().unwrap();
}

#[test]
fn shared_setgid_and_nonroot_sticky_bases_are_rejected_without_mutation() {
    let (_temporary, base, uid) = fixture();
    for mode in [0o2771, 0o770, 0o777, 0o1777] {
        if uid == 0 && mode == 0o1777 {
            continue;
        }
        fs::set_permissions(&base, fs::Permissions::from_mode(mode)).unwrap();
        let actual_mode = fs::metadata(&base).unwrap().mode() & 0o7777;
        let error = prepare_fixed_base(&base, uid, OhosRuntimePurpose::Aliases).unwrap_err();
        assert!(error.to_string().contains("validate-ancestor"));
        assert_eq!(fs::metadata(&base).unwrap().mode() & 0o7777, actual_mode);
        assert!(!base.join(format!("c{uid:08x}")).exists());
    }
    fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn wrong_effective_owner_is_rejected_even_with_private_mode() {
    let (_temporary, base, uid) = fixture();
    if uid != 0 {
        assert!(open_base(&base, uid.wrapping_add(1)).is_err());
    }
    assert!(
        validate_metadata(
            &base,
            &fs::metadata(&base).unwrap(),
            uid.wrapping_add(1),
            true
        )
        .is_err()
    );
    assert_eq!(fs::metadata(&base).unwrap().mode() & 0o7777, 0o700);
}

#[test]
fn every_symlink_component_is_rejected_without_touching_its_target() {
    let (_temporary, base, uid) = fixture();
    let target = base.join("target");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
    let link = base.join("link");
    symlink(&target, &link).unwrap();
    assert!(open_base(&link, uid).is_err());
    fs::create_dir(target.join("nested")).unwrap();
    assert!(open_base(&link.join("nested"), uid).is_err());
    symlink(&target, base.join(format!("c{uid:08x}"))).unwrap();
    assert!(prepare_fixed_base(&base, uid, OhosRuntimePurpose::Aliases).is_err());
    assert!(!target.join("a").exists());
    assert_eq!(fs::metadata(&target).unwrap().mode() & 0o7777, 0o700);
}

#[test]
fn existing_runtime_objects_are_never_repaired_or_replaced() {
    let (_temporary, base, uid) = fixture();
    let guard = prepare_fixed_base(&base, uid, OhosRuntimePurpose::Aliases).unwrap();
    let leaf = guard.path().to_path_buf();
    for mode in [0o755, 0o770, 0o2700, 0o1700] {
        fs::set_permissions(&leaf, fs::Permissions::from_mode(mode)).unwrap();
        let actual_mode = fs::metadata(&leaf).unwrap().mode() & 0o7777;
        if actual_mode != 0o700 {
            assert!(prepare_fixed_base(&base, uid, OhosRuntimePurpose::Aliases).is_err());
        }
        assert_eq!(fs::metadata(&leaf).unwrap().mode() & 0o7777, actual_mode);
    }
    fs::set_permissions(&leaf, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_dir(&leaf).unwrap();
    fs::write(&leaf, "keep this foreign object").unwrap();
    assert!(prepare_fixed_base(&base, uid, OhosRuntimePurpose::Aliases).is_err());
    assert_eq!(
        fs::read_to_string(&leaf).unwrap(),
        "keep this foreign object"
    );
}

#[test]
fn new_setgid_inheritance_is_normalized_through_its_descriptor_only() {
    let (_temporary, base, uid) = fixture();
    fs::set_permissions(&base, fs::Permissions::from_mode(0o2755)).unwrap();
    let actual_mode = fs::metadata(&base).unwrap().mode() & 0o7777;
    let guard = prepare_fixed_base(&base, uid, OhosRuntimePurpose::Aliases).unwrap();
    assert_eq!(fs::metadata(&base).unwrap().mode() & 0o7777, actual_mode);
    assert_eq!(fs::metadata(guard.path()).unwrap().mode() & 0o7777, 0o700);
    assert_eq!(
        fs::metadata(guard.path().parent().unwrap()).unwrap().mode() & 0o7777,
        0o700
    );
}

#[test]
fn pinned_chain_rejects_replacement_before_creating_more_objects() {
    let (_temporary, base, uid) = fixture();
    let guard = prepare_fixed_base(&base, uid, OhosRuntimePurpose::Aliases).unwrap();
    let user = base.join(format!("c{uid:08x}"));
    fs::rename(&user, base.join("original")).unwrap();
    fs::create_dir(&user).unwrap();
    fs::set_permissions(&user, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(user.join("a")).unwrap();
    fs::set_permissions(user.join("a"), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        guard
            .revalidate()
            .unwrap_err()
            .to_string()
            .contains("identity-changed")
    );
    assert!(
        guard
            .create_new_subdirectory(OsStr::new("must-not-exist"))
            .is_err()
    );
    assert!(!base.join("original/a/must-not-exist").exists());
    assert!(!user.join("a/must-not-exist").exists());
}

#[test]
fn runtime_initialization_converges_across_concurrent_callers() {
    let (_temporary, base, uid) = fixture();
    let identities = std::thread::scope(|scope| {
        let handles = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    let guard =
                        prepare_fixed_base(&base, uid, OhosRuntimePurpose::Aliases).unwrap();
                    let metadata = guard.leaf().file.metadata().unwrap();
                    (guard.path().to_path_buf(), metadata.dev(), metadata.ino())
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(identities.iter().all(|identity| identity == &identities[0]));
}

#[test]
fn lock_rejects_links_wrong_type_and_unsafe_modes_without_chmod() {
    let (_temporary, base, uid) = fixture();
    let guard = prepare_fixed_base(&base, uid, OhosRuntimePurpose::Aliases).unwrap();
    let file = guard.open_lock_file(OsStr::new("good.lock")).unwrap();
    assert_eq!(file.metadata().unwrap().mode() & 0o7777, 0o600);
    assert_ne!(
        unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    fs::set_permissions(
        guard.path().join("good.lock"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(guard.open_lock_file(OsStr::new("good.lock")).is_err());
    assert_eq!(
        fs::metadata(guard.path().join("good.lock")).unwrap().mode() & 0o7777,
        0o644
    );
    symlink(
        guard.path().join("good.lock"),
        guard.path().join("symlink.lock"),
    )
    .unwrap();
    assert!(guard.open_lock_file(OsStr::new("symlink.lock")).is_err());
    fs::hard_link(
        guard.path().join("good.lock"),
        guard.path().join("hard.lock"),
    )
    .unwrap();
    assert!(guard.open_lock_file(OsStr::new("hard.lock")).is_err());
    fs::create_dir(guard.path().join("directory.lock")).unwrap();
    assert!(guard.open_lock_file(OsStr::new("directory.lock")).is_err());
}

#[test]
fn socket_address_budget_counts_full_digest_separator_nul_and_utf8_bytes() {
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let maximum = address.sun_path.len() - 64 - 2;
    let fits = format!("/{}", "x".repeat(maximum - 1));
    check_socket_budget(Path::new(&fits)).unwrap();
    assert!(check_socket_budget(Path::new(&(fits + "x"))).is_err());
    assert!(check_socket_budget(Path::new(&format!("/{}", "界".repeat(maximum)))).is_err());
}

#[test]
fn invalid_or_missing_base_is_not_created() {
    let (_temporary, base, uid) = fixture();
    for bad in [
        PathBuf::from("/"),
        PathBuf::from("relative"),
        base.join("../escape"),
        base.join("./not-allowed"),
        base.join("absent"),
    ] {
        assert!(open_base(&bad, uid).is_err());
    }
    assert!(!base.join("absent").exists());
}

#[test]
fn runtime_environment_cannot_bind_or_replace_the_build_contract() {
    const CHILD: &str = "CODEX_UDS_RUNTIME_CONTRACT_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        assert_eq!(
            ohos_runtime_base_contract(),
            option_env!("CODEX_OHOS_RUNTIME_BASE")
        );
        if ohos_runtime_base_contract().is_none() {
            let error = prepare_ohos_runtime_directory(OhosRuntimePurpose::Aliases).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Unsupported);
            assert!(error.to_string().contains("stage=contract"));
        }
        return;
    }
    let (_temporary, base, _uid) = fixture();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ohos_runtime::tests::runtime_environment_cannot_bind_or_replace_the_build_contract",
        ])
        .env(CHILD, "1")
        .env("CODEX_OHOS_RUNTIME_BASE", &base)
        .env("HOME", &base)
        .env("CODEX_HOME", &base)
        .env("TMPDIR", &base)
        .env("XDG_RUNTIME_DIR", &base)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_dir(base).unwrap().count(), 0);
}

#[test]
fn reported_device_identity_and_setgid_are_not_sticky_protection() {
    assert!(!directory_mode_is_safe(20001006, 20020101, 0o2771, false));
    assert!(!directory_mode_is_safe(20020101, 20020101, 0o2771, false));
    assert!(!directory_mode_is_safe(20020101, 20020101, 0o1777, false));
    assert!(directory_mode_is_safe(0, 20020101, 0o1777, false));
    assert!(!directory_mode_is_safe(0, 20020101, 0o2777, false));
    assert!(!directory_mode_is_safe(20020101, 20020101, 0o2700, true));
    assert!(directory_mode_is_safe(20020101, 20020101, 0o700, true));
}

#[test]
fn replaced_lock_and_every_cloned_descriptor_are_checked() {
    let (_temporary, base, uid) = fixture();
    let parent = prepare_fixed_base(&base, uid, OhosRuntimePurpose::Aliases).unwrap();
    let child = parent
        .create_new_subdirectory(OsStr::new("session"))
        .unwrap();
    for directory in &child.directories {
        assert_ne!(
            unsafe { libc::fcntl(directory.file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
    }
    let file = child.open_lock_file(OsStr::new("session.lock")).unwrap();
    fs::remove_file(child.path().join("session.lock")).unwrap();
    let replacement = child.open_lock_file(OsStr::new("session.lock")).unwrap();
    assert!(
        child
            .validate_lock_file(OsStr::new("session.lock"), &file)
            .is_err()
    );
    child
        .validate_lock_file(OsStr::new("session.lock"), &replacement)
        .unwrap();
}

#[test]
fn private_directory_compatibility_entry_never_repairs_existing_modes() {
    let (_temporary, base, _uid) = fixture();
    let path = base.join("private-socket-parent");
    prepare_private_directory(&path)
        .unwrap()
        .revalidate()
        .unwrap();
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o700);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o770)).unwrap();
    assert!(prepare_private_directory(&path).is_err());
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o770);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn read_only_parent_fails_without_changing_it() {
    let (_temporary, base, uid) = fixture();
    if uid == 0 {
        eprintln!("read-only mode test needs an unprivileged user");
        return;
    }
    fs::set_permissions(&base, fs::Permissions::from_mode(0o500)).unwrap();
    let result = prepare_fixed_base(&base, uid, OhosRuntimePurpose::Aliases);
    let mode = fs::metadata(&base).unwrap().mode() & 0o7777;
    let children = fs::read_dir(&base).unwrap().count();
    fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(mode, 0o500);
    assert_eq!(children, 0);
}
