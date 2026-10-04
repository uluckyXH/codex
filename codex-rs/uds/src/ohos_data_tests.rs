use super::*;
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    #[cfg(target_os = "macos")]
    let temporary = tempfile::Builder::new()
        .prefix("cd")
        .tempdir_in("/private/tmp")
        .unwrap();
    #[cfg(not(target_os = "macos"))]
    let temporary = tempfile::Builder::new().prefix("cd").tempdir().unwrap();
    let files = temporary.path().canonicalize().unwrap();
    fs::set_permissions(&files, fs::Permissions::from_mode(0o700)).unwrap();
    (temporary, files)
}

fn initialize(files: &Path) -> io::Result<OhosDataDirectories> {
    initialize_at(
        files,
        OhosProcessIdentity::current(),
        OhosDirectorySource::ApplicationContext,
        None,
    )
}

#[test]
fn first_start_creates_the_complete_private_layout_and_preserves_cwd() {
    let (_temporary, files) = fixture();
    let cwd = std::env::current_dir().unwrap();
    let directories = initialize(&files).unwrap();
    assert_eq!(directories.state_dir(), files.join("codex/state"));
    assert_eq!(directories.runtime_dir(), files.join("codex/r"));
    assert_eq!(directories.logs_dir(), files.join("codex/logs"));
    assert_eq!(directories.temp_dir(), files.join("codex/tmp"));
    for path in [
        "codex",
        "codex/state",
        "codex/r",
        "codex/r/a",
        "codex/r/s",
        "codex/tmp",
        "codex/logs",
    ] {
        let metadata = fs::metadata(files.join(path)).unwrap();
        assert_eq!(metadata.uid(), OhosProcessIdentity::current().euid);
        assert_eq!(metadata.mode() & 0o7777, 0o700);
    }
    assert!(
        !directories
            .runtime_dir()
            .join(format!("c{:08x}", directories.identity().euid))
            .exists()
    );
    assert_eq!(std::env::current_dir().unwrap(), cwd);
    let sockets = directories
        .prepare_runtime_directory(OhosRuntimePurpose::ControlSockets)
        .unwrap();
    assert_eq!(sockets.path(), files.join("codex/r/s"));
    for directory in &sockets.directories {
        assert_ne!(
            unsafe { libc::fcntl(directory.file.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
    }
}

#[test]
fn repeated_and_concurrent_initialization_preserve_objects_and_configuration() {
    let (_temporary, files) = fixture();
    let first = initialize(&files).unwrap();
    let config = first.state_dir().join("config.toml");
    fs::write(&config, "model = \"gpt-5.6-terra\"\n").unwrap();
    let root_inode = fs::metadata(first.root_dir()).unwrap().ino();
    let sockets_inode = fs::metadata(first.runtime_dir().join("s")).unwrap().ino();
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    let directories = initialize(&files).unwrap();
                    assert_eq!(
                        fs::metadata(directories.root_dir()).unwrap().ino(),
                        root_inode
                    );
                    assert_eq!(
                        fs::metadata(directories.runtime_dir().join("s"))
                            .unwrap()
                            .ino(),
                        sockets_inode
                    );
                    directories.revalidate().unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    });
    assert_eq!(
        fs::read_to_string(config).unwrap(),
        "model = \"gpt-5.6-terra\"\n"
    );
    first.revalidate().unwrap();
}

#[test]
fn initial_shared_files_and_foreign_owner_are_rejected_without_a_child_or_chmod() {
    let (_temporary, files) = fixture();
    // A Mac /private/tmp child can inherit group wheel. Unprivileged chmod
    // silently clears setgid when the caller is not in that group, so first
    // give this disposable fixture the actual process group.
    let file = fs::File::open(&files).unwrap();
    assert_eq!(
        unsafe { libc::fchown(file.as_raw_fd(), libc::geteuid(), libc::getegid()) },
        0
    );
    for mode in [0o755, 0o770, 0o777, 0o1777, 0o2700] {
        fs::set_permissions(&files, fs::Permissions::from_mode(mode)).unwrap();
        let original = fs::metadata(&files).unwrap().mode() & 0o7777;
        assert_eq!(
            original, mode,
            "fixture must actually have the rejected mode"
        );
        assert!(initialize(&files).is_err(), "{mode:o}");
        assert_eq!(fs::metadata(&files).unwrap().mode() & 0o7777, original);
        assert!(!files.join("codex").exists());
    }
    fs::set_permissions(&files, fs::Permissions::from_mode(0o700)).unwrap();
    let mut foreign = OhosProcessIdentity::current();
    foreign.euid = foreign.euid.wrapping_add(1);
    assert!(
        initialize_at(
            &files,
            foreign,
            OhosDirectorySource::ApplicationContext,
            None
        )
        .is_err()
    );
    assert!(!files.join("codex").exists());
}

#[test]
fn a_private_files_leaf_cannot_exempt_a_shared_application_ancestor() {
    let (_temporary, parent) = fixture();
    // One short extra component stays within the Mac socket-address budget.
    let files = parent.join("f");
    fs::create_dir(&files).unwrap();
    fs::set_permissions(&files, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).unwrap();
    let error = initialize(&files).unwrap_err();
    assert!(error.to_string().contains("validate-ancestor"));
    assert!(!files.join("codex").exists());
    assert_eq!(fs::metadata(&parent).unwrap().mode() & 0o7777, 0o777);
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn unsafe_existing_subdirectory_is_preserved_and_failure_can_be_retried() {
    let (_temporary, files) = fixture();
    let directories = initialize(&files).unwrap();
    let state = directories.state_dir().to_path_buf();
    fs::write(state.join("config.toml"), "model = \"gpt-5.6-terra\"\n").unwrap();
    drop(directories);
    fs::set_permissions(&state, fs::Permissions::from_mode(0o770)).unwrap();
    assert!(initialize(&files).is_err());
    assert_eq!(fs::metadata(&state).unwrap().mode() & 0o7777, 0o770);
    assert_eq!(
        fs::read_to_string(state.join("config.toml")).unwrap(),
        "model = \"gpt-5.6-terra\"\n"
    );
    // Only the fixture owner repairs the rejected fixture, never initialization.
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    initialize(&files).unwrap().revalidate().unwrap();
}

#[test]
fn symlink_file_and_replaced_data_root_are_never_adopted() {
    let (_temporary, files) = fixture();
    let root = files.join("codex");
    fs::write(&root, "keep").unwrap();
    assert!(initialize(&files).is_err());
    assert_eq!(fs::read_to_string(&root).unwrap(), "keep");
    fs::remove_file(&root).unwrap();
    symlink(&files, &root).unwrap();
    assert!(initialize(&files).is_err());
    fs::remove_file(&root).unwrap();
    let directories = initialize(&files).unwrap();
    let original = files.join("original");
    fs::rename(&root, &original).unwrap();
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(directories.revalidate().is_err());
    assert!(
        directories
            .prepare_runtime_directory(OhosRuntimePurpose::ControlSockets)
            .is_err()
    );
    assert_eq!(fs::read_dir(root).unwrap().count(), 0);
    assert!(original.join("state").is_dir());
}

#[test]
fn overlong_files_path_is_rejected_before_any_data_root_is_created() {
    let (_temporary, parent) = fixture();
    let files = parent.join("long-platform-files-directory");
    fs::create_dir(&files).unwrap();
    fs::set_permissions(&files, fs::Permissions::from_mode(0o700)).unwrap();
    let error = initialize(&files).unwrap_err();
    assert!(error.to_string().contains("socket-path-budget"));
    assert!(!files.join("codex").exists());
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let root_bytes = address.sun_path.len() - 1 - 64 - 1;
    let limit = PathBuf::from(format!("/{}", "x".repeat(root_bytes - 1)));
    check_socket_budget(&limit).unwrap();
    assert!(check_socket_budget(&limit.join("x")).is_err());
}

#[test]
fn real_and_effective_uid_and_gid_are_independent_facts() {
    // This is a policy fixture, not evidence of another actual device identity.
    let identity = OhosProcessIdentity {
        uid: 20020060,
        euid: 20020061,
        gid: 4000,
        egid: 4001,
    };
    identity.validate_current(identity).unwrap();
    for changed in [
        OhosProcessIdentity {
            uid: identity.uid + 1,
            ..identity
        },
        OhosProcessIdentity {
            euid: identity.euid + 1,
            ..identity
        },
        OhosProcessIdentity {
            gid: identity.gid + 1,
            ..identity
        },
        OhosProcessIdentity {
            egid: identity.egid + 1,
            ..identity
        },
    ] {
        assert!(identity.validate_current(changed).is_err());
    }
    assert!(super::super::directory_profile_mode_is_safe(
        Path::new("/private/root"),
        identity.euid,
        identity.egid,
        identity.euid,
        0o700,
        true,
        RuntimeProfile::Strict
    ));
}

#[test]
fn unavailable_context_selects_only_the_fixed_platform_candidate() {
    let (files, source, reason) = select_files_root(context::ContextFiles::Unavailable(
        "context_not_exist".into(),
    ))
    .unwrap();
    assert_eq!(files, Path::new(ohos_platform_files_candidate()));
    assert_eq!(source, OhosDirectorySource::ValidatedPlatformNamespace);
    assert_eq!(reason.as_deref(), Some("context_not_exist"));
    assert!(select_files_root(context::ContextFiles::Available(PathBuf::from("/a/../b"))).is_err());
    let (_temporary, files) = fixture();
    let (selected, source, reason) =
        select_files_root(context::ContextFiles::Available(files.clone())).unwrap();
    assert_eq!(selected, files);
    assert_eq!(source, OhosDirectorySource::ApplicationContext);
    assert_eq!(reason, None);
}

#[tokio::test]
async fn short_runtime_keeps_complete_names_locking_and_owned_socket_cleanup() {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    let (_temporary, files) = fixture();
    let directories = initialize(&files).unwrap();
    let name = "a".repeat(64);
    let (mut listener, guard) = directories
        .prepare_runtime_directory(OhosRuntimePurpose::ControlSockets)
        .unwrap()
        .bind_control_socket(OsStr::new(&name))
        .await
        .unwrap();
    let path = guard.path();
    assert_eq!(path, files.join("codex/r/s").join(&name));
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o600);
    assert_eq!(
        fs::metadata(files.join("codex/r/s").join(format!("{name}.lock")))
            .unwrap()
            .mode()
            & 0o7777,
        0o600
    );
    let duplicate = directories
        .prepare_runtime_directory(OhosRuntimePurpose::ControlSockets)
        .unwrap()
        .bind_control_socket(OsStr::new(&name))
        .await;
    assert!(matches!(duplicate, Err(error) if error.kind() == io::ErrorKind::AddrInUse));
    // The duplicate's liveness probe makes the first queued connection.
    drop(listener.accept().await.unwrap());
    let mut client = crate::UnixStream::connect(&path).await.unwrap();
    let mut server = listener.accept().await.unwrap();
    client.write_all(b"short-root").await.unwrap();
    let mut reply = [0; 10];
    server.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"short-root");
    drop(client);
    drop(server);
    drop(listener);
    drop(guard);
    assert!(!path.exists());
    assert!(
        files
            .join("codex/r/s")
            .join(format!("{name}.lock"))
            .is_file()
    );
    directories.revalidate().unwrap();
}
