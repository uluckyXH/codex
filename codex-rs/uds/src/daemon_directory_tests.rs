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
