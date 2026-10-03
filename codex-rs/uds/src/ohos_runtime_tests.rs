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
        assert_eq!(
            ohos_runtime_profile_contract(),
            option_env!("CODEX_OHOS_RUNTIME_PROFILE").unwrap_or("strict")
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
        .env("CODEX_OHOS_RUNTIME_PROFILE", "hdc-debug")
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
fn hdc_debug_contract_requires_an_explicit_fixed_base_and_shell_identity() {
    let base = Path::new(HDC_DEBUG_BASE);
    assert_eq!(
        RuntimeProfile::from_contract("strict", base, 2000, None).unwrap(),
        RuntimeProfile::Strict
    );
    assert_eq!(
        RuntimeProfile::from_contract("hdc-debug", base, 2000, None).unwrap(),
        RuntimeProfile::HdcDebug
    );
    for (profile, path, uid) in [
        ("automatic", HDC_DEBUG_BASE, 2000),
        ("hdc-debug", "/data/local/tmp", 2000),
        ("hdc-debug", "/data/local/tmp/cdx-other", 2000),
        ("hdc-debug", "/data/storage/el2/base/files", 2000),
        ("hdc-debug", HDC_DEBUG_BASE, 0),
        ("hdc-debug", HDC_DEBUG_BASE, 20020101),
    ] {
        let error = RuntimeProfile::from_contract(profile, Path::new(path), uid, None).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("profile-contract"));
    }
    RuntimeProfile::HdcDebug
        .validate_scope(&base.join("c000007d0/a"), 2000)
        .unwrap();
    assert!(
        RuntimeProfile::HdcDebug
            .validate_scope(Path::new("/data/local/tmp/cdx-other"), 2000)
            .is_err()
    );
}

#[test]
fn hdc_debug_platform_ancestors_require_exact_paths_owners_groups_and_modes() {
    for (path, owner, group, mode) in [
        ("/data", 1000, 1000, 0o771),
        ("/data/local", 0, 0, 0o751),
        ("/data/local/tmp", 2000, 2000, 0o771),
    ] {
        let path = Path::new(path);
        let accepts = |owner, group, uid, mode, private| {
            directory_profile_mode_is_safe(
                path,
                owner,
                group,
                uid,
                mode,
                private,
                RuntimeProfile::HdcDebug,
            )
        };
        assert!(accepts(owner, group, 2000, mode, false));
        assert!(!accepts(owner + 1, group, 2000, mode, false));
        assert!(!accepts(owner, group + 1, 2000, mode, false));
        assert!(!accepts(owner, group, 20020101, mode, false));
        assert!(!accepts(owner, group, 2000, mode | 0o002, false));
        assert!(!accepts(owner, group, 2000, mode | 0o2000, false));
        assert!(!accepts(owner, group, 2000, mode, true));
    }
    // The same ownership and mode at any other pathname receive no exception.
    for path in [
        "/data-copy",
        "/data/local/tmp-other",
        "/data/local/tmp/cdx/shared",
    ] {
        assert!(!directory_profile_mode_is_safe(
            Path::new(path),
            2000,
            2000,
            2000,
            0o771,
            false,
            RuntimeProfile::HdcDebug,
        ));
    }
    assert!(!directory_profile_mode_is_safe(
        Path::new("/data"),
        1000,
        1000,
        2000,
        0o771,
        false,
        RuntimeProfile::Strict,
    ));
    assert!(!directory_profile_mode_is_safe(
        Path::new("/data/local/tmp"),
        2000,
        2000,
        2000,
        0o771,
        false,
        RuntimeProfile::Strict,
    ));
}

#[test]
fn hnp_debug_contract_binds_the_installation_uid_and_fixed_private_root() {
    let uid = 20020059;
    let base = Path::new(HNP_DEBUG_BASE);
    let profile = RuntimeProfile::from_contract("hnp-debug", base, uid, Some("20020059"))
        .expect("explicit installation contract");
    assert_eq!(profile, RuntimeProfile::HnpDebug { uid });
    for value in [
        None,
        Some(""),
        Some("0"),
        Some("2000"),
        Some("1000"),
        Some("+20020059"),
        Some("20020059 "),
        Some("20020060"),
        Some("4294967296"),
    ] {
        assert!(RuntimeProfile::from_contract("hnp-debug", base, uid, value).is_err());
    }
    for other_uid in [0, 1000, 2000, 20020060] {
        assert!(
            RuntimeProfile::from_contract("hnp-debug", base, other_uid, Some("20020059")).is_err()
        );
        assert!(profile.validate_scope(base, other_uid).is_err());
    }
    for platform_uid in [0, 1000, 2000, 9999] {
        assert!(
            RuntimeProfile::from_contract(
                "hnp-debug",
                base,
                platform_uid,
                Some(&platform_uid.to_string())
            )
            .is_err()
        );
    }
    for other_base in [
        "/data/storage/el2/base/files",
        "/data/storage/el2/base/files/r-other",
        HDC_DEBUG_BASE,
    ] {
        assert!(
            RuntimeProfile::from_contract(
                "hnp-debug",
                Path::new(other_base),
                uid,
                Some("20020059")
            )
            .is_err()
        );
        assert!(profile.validate_scope(Path::new(other_base), uid).is_err());
    }
    profile
        .validate_scope(&base.join(format!("c{uid:08x}/a")), uid)
        .unwrap();
}

#[test]
fn hnp_debug_never_exempts_shared_context_ancestors_or_foreign_owners() {
    let uid = 20020059;
    let profile = RuntimeProfile::HnpDebug { uid };
    for path in [
        "/data/storage/el2/base",
        "/data/storage/el2/base/files",
        HNP_DEBUG_BASE,
    ] {
        let path = Path::new(path);
        assert!(directory_profile_mode_is_safe(
            path, uid, uid, uid, 0o700, false, profile
        ));
        for mode in [0o770, 0o777, 0o2771, 0o1777] {
            assert!(!directory_profile_mode_is_safe(
                path, uid, uid, uid, mode, false, profile
            ));
        }
        assert!(!directory_profile_mode_is_safe(
            path,
            uid + 1,
            uid,
            uid,
            0o700,
            false,
            profile
        ));
        assert!(!directory_profile_mode_is_safe(
            path,
            uid,
            uid,
            uid + 1,
            0o700,
            false,
            profile
        ));
    }
    assert!(!directory_profile_mode_is_safe(
        Path::new(HNP_DEBUG_BASE),
        uid,
        uid,
        uid,
        0o755,
        false,
        profile
    ));
}

#[test]
fn hdc_debug_private_base_and_leaves_keep_strict_private_permissions() {
    for path in [HDC_DEBUG_BASE, "/data/local/tmp/cdx/c000007d0/a"] {
        for mode in [0o700, 0o755, 0o770, 0o771, 0o777, 0o1700, 0o2700] {
            assert_eq!(
                directory_profile_mode_is_safe(
                    Path::new(path),
                    2000,
                    2000,
                    2000,
                    mode,
                    true,
                    RuntimeProfile::HdcDebug,
                ),
                mode == 0o700
            );
        }
    }
    assert!(!directory_profile_mode_is_safe(
        Path::new(HDC_DEBUG_BASE),
        2000,
        2000,
        2000,
        0o755,
        false,
        RuntimeProfile::HdcDebug,
    ));
}

#[cfg(target_env = "ohos")]
#[test]
#[ignore = "requires an explicitly compiled hdc-debug test binary and precreated 0700 base"]
fn hdc_debug_profile_prepares_private_runtime_on_target() {
    assert_eq!(ohos_runtime_profile_contract(), "hdc-debug");
    assert_eq!(ohos_runtime_base_contract(), Some(HDC_DEBUG_BASE));
    assert_eq!(unsafe { libc::geteuid() }, HDC_SHELL_UID);
    assert!(open_base(Path::new(HDC_DEBUG_BASE), HDC_SHELL_UID).is_err());
    let aliases = prepare_ohos_runtime_directory(OhosRuntimePurpose::Aliases).unwrap();
    assert_eq!(aliases.path(), Path::new("/data/local/tmp/cdx/c000007d0/a"));
    let child_name = format!("udstest-{}", std::process::id());
    let child = aliases
        .create_new_subdirectory(OsStr::new(&child_name))
        .unwrap();
    let lock = child.open_lock_file(OsStr::new("probe.lock")).unwrap();
    lock.lock().unwrap();
    child
        .validate_lock_file(OsStr::new("probe.lock"), &lock)
        .unwrap();
    let link = child.path().join("symlink");
    symlink(aliases.path(), &link).unwrap();
    assert!(validate_ohos_runtime_base(&link).is_err());
    fs::remove_file(link).unwrap();
    drop(lock);
    child.remove_child(OsStr::new("probe.lock")).unwrap();
    let child_path = child.path().to_path_buf();
    drop(child);
    fs::remove_dir(child_path).unwrap();
    aliases.revalidate().unwrap();
    let sockets = prepare_ohos_runtime_directory(OhosRuntimePurpose::ControlSockets).unwrap();
    sockets.revalidate().unwrap();
    assert_eq!(sockets.path(), Path::new("/data/local/tmp/cdx/c000007d0/s"));
}

#[cfg(target_env = "ohos")]
#[tokio::test(flavor = "current_thread")]
#[ignore = "requires hdc-debug deployment and OS permission to bind pathname Unix sockets"]
async fn hdc_debug_profile_socket_lifecycle_on_target() {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    assert_eq!(ohos_runtime_profile_contract(), "hdc-debug");
    let sockets = prepare_ohos_runtime_directory(OhosRuntimePurpose::ControlSockets).unwrap();
    let name = format!("{:064x}", std::process::id());
    let lock_path = sockets.path().join(format!("{name}.lock"));
    let (mut listener, guard) = sockets
        .bind_control_socket(OsStr::new(&name))
        .await
        .unwrap();
    let socket_path = guard.path();
    let mut client = crate::UnixStream::connect(&socket_path).await.unwrap();
    let mut server = listener.accept().await.unwrap();
    client.write_all(b"runtime-ok").await.unwrap();
    let mut reply = [0; 10];
    server.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"runtime-ok");
    drop(client);
    drop(server);
    drop(listener);
    drop(guard);
    assert!(!socket_path.exists());
    fs::remove_file(lock_path).unwrap();
}

#[cfg(target_env = "ohos")]
#[test]
#[ignore = "requires an installation-bound HNP debug build and preprovisioned private application root"]
fn hnp_debug_profile_prepares_private_runtime_on_target() {
    assert_eq!(ohos_runtime_profile_contract(), "hnp-debug");
    assert_eq!(ohos_runtime_base_contract(), Some(HNP_DEBUG_BASE));
    let uid = unsafe { libc::geteuid() };
    assert_eq!(
        ohos_runtime_uid_contract().unwrap().parse::<u32>().unwrap(),
        uid
    );
    let aliases = prepare_ohos_runtime_directory(OhosRuntimePurpose::Aliases).unwrap();
    let expected = Path::new(HNP_DEBUG_BASE).join(format!("c{uid:08x}"));
    assert_eq!(aliases.path(), expected.join("a"));
    let child = aliases
        .create_new_subdirectory(OsStr::new(&format!("hnptest-{}", std::process::id())))
        .unwrap();
    let lock = child.open_lock_file(OsStr::new("probe.lock")).unwrap();
    lock.lock().unwrap();
    child
        .validate_lock_file(OsStr::new("probe.lock"), &lock)
        .unwrap();
    drop(lock);
    child.remove_child(OsStr::new("probe.lock")).unwrap();
    let child_path = child.path().to_path_buf();
    drop(child);
    fs::remove_dir(child_path).unwrap();
    aliases.revalidate().unwrap();
    let sockets = prepare_ohos_runtime_directory(OhosRuntimePurpose::ControlSockets).unwrap();
    assert_eq!(sockets.path(), expected.join("s"));
    sockets.revalidate().unwrap();
}

#[cfg(target_env = "ohos")]
#[test]
#[ignore = "measures symlink creation policy of the dedicated HNP debug application"]
fn hnp_debug_profile_cannot_create_symlinks_on_target() {
    assert_eq!(ohos_runtime_profile_contract(), "hnp-debug");
    let aliases = prepare_ohos_runtime_directory(OhosRuntimePurpose::Aliases).unwrap();
    let child = aliases
        .create_new_subdirectory(OsStr::new(&format!("hnplink-{}", std::process::id())))
        .unwrap();
    let link = child.path().join("symlink");
    let error = symlink(aliases.path(), &link).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    child.revalidate().unwrap();
    let child_path = child.path().to_path_buf();
    drop(child);
    fs::remove_dir(child_path).unwrap();
}

#[cfg(target_env = "ohos")]
#[tokio::test(flavor = "current_thread")]
#[ignore = "requires an HNP debug deployment with OS permission to bind pathname Unix sockets"]
async fn hnp_debug_profile_socket_lifecycle_on_target() {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    assert_eq!(ohos_runtime_profile_contract(), "hnp-debug");
    let sockets = prepare_ohos_runtime_directory(OhosRuntimePurpose::ControlSockets).unwrap();
    let name = format!("{:064x}", std::process::id());
    let lock_path = sockets.path().join(format!("{name}.lock"));
    let (mut listener, guard) = sockets
        .bind_control_socket(OsStr::new(&name))
        .await
        .unwrap();
    let socket_path = guard.path();
    let mut client = crate::UnixStream::connect(&socket_path).await.unwrap();
    let mut server = listener.accept().await.unwrap();
    client.write_all(b"hnp-runtime-ok").await.unwrap();
    let mut reply = [0; 14];
    server.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"hnp-runtime-ok");
    drop(client);
    drop(server);
    drop(listener);
    drop(guard);
    assert!(!socket_path.exists());
    fs::remove_file(lock_path).unwrap();
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

#[test]
fn new_private_directory_keeps_a_chmod_capable_descriptor() {
    let (_temporary, base, uid) = fixture();
    let guard = prepare_fixed_base(&base, uid, OhosRuntimePurpose::Aliases).unwrap();
    // Even a no-op fchmod would fail with EBADF on an O_PATH descriptor.
    set_new_mode(&guard.leaf().file, guard.path(), 0o700).unwrap();
    guard.revalidate().unwrap();
    let child = guard
        .create_new_subdirectory(OsStr::new("cloned-chain"))
        .unwrap();
    for directory in &child.directories {
        let flags = unsafe { libc::fcntl(directory.file.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0);
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
        #[cfg(target_env = "ohos")]
        {
            let status = unsafe { libc::fcntl(directory.file.as_raw_fd(), libc::F_GETFL) };
            assert!(status >= 0);
            assert_eq!(
                status & libc::O_PATH,
                if directory.private { 0 } else { libc::O_PATH }
            );
        }
    }
    let file = child.open_lock_file(OsStr::new("relative.lock")).unwrap();
    file.lock().unwrap();
    child
        .validate_lock_file(OsStr::new("relative.lock"), &file)
        .unwrap();
}

// macOS has no O_PATH. Compile/link this test for OHOS; only an actual OHOS
// test execution can verify the platform's path-only descriptor semantics.
#[cfg(target_env = "ohos")]
#[test]
fn search_only_ancestor_supports_path_handles_without_weakening_validation() {
    let (_temporary, base, uid) = fixture();
    let ancestor = base.join("search-only");
    let candidate = ancestor.join("known-private-child");
    fs::create_dir(&ancestor).unwrap();
    fs::create_dir(&candidate).unwrap();
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o700)).unwrap();
    struct RestorePermissions(PathBuf);
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
        }
    }
    let _restore = RestorePermissions(ancestor.clone());
    // Owned 0111 gives an unprivileged test user the same lack of read access
    // as a root-owned 0711 ancestor, without chown or elevated fixtures.
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o111)).unwrap();
    if uid != 0 {
        assert_eq!(
            File::open(&ancestor).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    let base_guard = open_base(&candidate, uid).unwrap();
    let flags = unsafe { libc::fcntl(base_guard.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0);
    assert_ne!(flags & libc::O_PATH, 0);
    let lock = base_guard
        .open_lock_file(OsStr::new("path-handle.lock"))
        .unwrap();
    lock.lock().unwrap();
    base_guard
        .validate_lock_file(OsStr::new("path-handle.lock"), &lock)
        .unwrap();
    let guard = prepare_fixed_base(&candidate, uid, OhosRuntimePurpose::Aliases).unwrap();
    guard.revalidate().unwrap();
    let child = guard
        .create_new_subdirectory(OsStr::new("session"))
        .unwrap();
    child.revalidate().unwrap();
    set_new_mode(&child.leaf().file, child.path(), 0o700).unwrap();
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o771)).unwrap();
    assert!(guard.revalidate().is_err());
    assert_eq!(fs::metadata(&ancestor).unwrap().mode() & 0o7777, 0o771);
    assert!(
        child
            .create_new_subdirectory(OsStr::new("must-not-create"))
            .is_err()
    );
}
