use super::*;
use pretty_assertions::assert_eq;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;

fn uid() -> libc::uid_t {
    unsafe { libc::geteuid() }
}

#[test]
fn private_reservation_is_idempotent_and_concurrent() -> io::Result<()> {
    let temp = tempfile::tempdir()?;
    let directory = temp.path().join(".codex-uds");
    std::thread::scope(|scope| {
        let handles = (0..8)
            .map(|_| scope.spawn(|| prepare_directory(&directory, uid())))
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().expect("prepare thread")?;
        }
        Ok::<_, io::Error>(())
    })?;
    let metadata = fs::symlink_metadata(&directory)?;
    assert_eq!(metadata.mode() & 0o7777, 0o700);
    assert_eq!(metadata.uid(), uid());
    let marker = directory.join("active-socket-marker");
    fs::write(&marker, "still active")?;
    prepare_directory(&directory, uid())?;
    assert_eq!(fs::read_to_string(marker)?, "still active");
    Ok(())
}

#[test]
fn reservation_rejects_symlinks_files_wrong_owner_and_unsafe_modes() -> io::Result<()> {
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("target");
    fs::create_dir(&target)?;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700))?;
    let link = temp.path().join("link");
    symlink(&target, &link)?;
    assert!(prepare_directory(&link, uid()).is_err());
    let file = temp.path().join("file");
    fs::write(&file, "keep")?;
    assert!(prepare_directory(&file, uid()).is_err());
    assert!(prepare_directory(&target, uid().wrapping_add(1)).is_err());
    for mode in [0o755, 0o770, 0o600, 0o1700] {
        fs::set_permissions(&target, fs::Permissions::from_mode(mode))?;
        assert!(prepare_directory(&target, uid()).is_err(), "mode {mode:o}");
        assert_eq!(fs::metadata(&target)?.mode() & 0o7777, mode);
    }
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700))?;
    assert_eq!(fs::read_to_string(file)?, "keep");
    Ok(())
}

#[test]
fn account_home_canonicalization_rejects_invalid_leaf_identity() -> io::Result<()> {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let home = temp.path().join("account");
    fs::create_dir(&home)?;
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700))?;
    let alias = temp.path().join("account-alias");
    symlink(&home, &alias)?;
    assert_eq!(
        canonical_account_home(&alias, uid())?,
        fs::canonicalize(&home)?
    );
    assert!(canonical_account_home(&home, uid().wrapping_add(1)).is_err());
    assert!(canonical_account_home(Path::new("/"), uid()).is_err());
    assert!(canonical_account_home(Path::new("relative"), uid()).is_err());
    fs::set_permissions(&home, fs::Permissions::from_mode(0o777))?;
    assert!(canonical_account_home(&home, uid()).is_err());
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o777))?;
    let error = trusted_account_home(&home, uid()).expect_err("unsafe ancestor");
    assert!(
        error
            .to_string()
            .contains(&temp.path().display().to_string())
    );
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[test]
fn socket_budget_includes_full_transport_digest_and_utf8_bytes() {
    assert!(
        check_socket_path_length(&Path::new(OHOS_CURRENT_USER_HOME).join(".codex-uds")).is_ok()
    );
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let max_directory_bytes = address.sun_path.len() - 1 - 1 - 64;
    let fits = format!("/{}", "x".repeat(max_directory_bytes - 1));
    assert!(check_socket_path_length(Path::new(&fits)).is_ok());
    assert!(check_socket_path_length(Path::new(&(fits + "x"))).is_err());
    assert!(check_socket_path_length(Path::new(&format!("/{}", "界".repeat(30)))).is_err());
}

#[test]
fn existing_platform_entry_never_falls_back_after_failed_validation() -> io::Result<()> {
    let temp = tempfile::tempdir()?;
    let dangling = temp.path().join("current-user");
    symlink(temp.path().join("missing"), &dangling)?;
    let wrong_type = temp.path().join("file");
    fs::write(&wrong_type, "keep")?;
    let unsafe_home = temp.path().join("unsafe-home");
    fs::create_dir(&unsafe_home)?;
    fs::set_permissions(&unsafe_home, fs::Permissions::from_mode(0o777))?;
    for path in [&dangling, &wrong_type, &unsafe_home] {
        assert!(
            ohos_home(path, uid(), |_| panic!(
                "existing platform entry must not query passwd"
            ))
            .is_err()
        );
    }
    fs::set_permissions(&unsafe_home, fs::Permissions::from_mode(0o700))?;
    assert!(
        ohos_home(&unsafe_home, uid().wrapping_add(1), |_| {
            panic!("wrong owner must not query passwd")
        })
        .is_err()
    );
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o777))?;
    assert!(
        ohos_home(&unsafe_home, uid(), |_| {
            panic!("unsafe ancestor must not query passwd")
        })
        .is_err()
    );
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[test]
fn absent_platform_entry_requires_a_trusted_account_home() -> io::Result<()> {
    let temp = tempfile::tempdir()?;
    let missing = temp.path().join("missing");
    let error = ohos_home(&missing, uid(), |requested_uid| {
        assert_eq!(requested_uid, uid());
        Err(io::Error::new(io::ErrorKind::NotFound, "no account entry"))
    })
    .expect_err("missing account must not use the environment");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(error.to_string().contains("no account entry"));
    for home in [Path::new("/"), Path::new(""), Path::new("relative")] {
        assert!(ohos_home(&missing, uid(), |_| Ok(home.to_path_buf())).is_err());
    }
    // A returned account path still gets the complete filesystem validation.
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o777))?;
    assert!(ohos_home(&missing, uid(), |_| Ok(temp.path().to_path_buf())).is_err());
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[test]
fn directory_errors_include_stage_path_and_os_error() -> io::Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("missing/daemon");
    let error = prepare_directory(&path, uid()).expect_err("missing parent");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    let message = error.to_string();
    assert!(message.contains("open reserved directory parent"));
    assert!(message.contains(&path.parent().unwrap().display().to_string()));
    assert!(message.contains("os error"));
    Ok(())
}

#[test]
fn account_lookup_ignores_tool_environment() -> io::Result<()> {
    const EXPECTED: &str = "CODEX_UDS_TEST_ACCOUNT_HOME";
    if let Some(expected) = std::env::var_os(EXPECTED) {
        assert_eq!(account_home(uid())?, PathBuf::from(expected));
        return Ok(());
    }
    let expected = account_home(uid())?;
    let output = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "daemon_directory::tests::account_lookup_ignores_tool_environment",
        ])
        .env(EXPECTED, expected)
        .env("HOME", "/untrusted-tool-home")
        .env("TMPDIR", "/untrusted-tool-tmp")
        .env("CODEX_HOME", "/untrusted-tool-codex")
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
