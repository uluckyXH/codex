//! Socket lifecycle under a pinned private runtime directory. No public alias.

use crate::ProtectedRuntimeDirectory;
use crate::UnixListener;
use crate::UnixStream;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::io;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

/// Keeps the directory pinned and removes only the socket created by this run.
pub struct ProtectedControlSocket {
    directory: ProtectedRuntimeDirectory,
    name: OsString,
    identity: SocketIdentity,
}

impl ProtectedControlSocket {
    pub fn path(&self) -> PathBuf {
        self.directory.path().join(&self.name)
    }
}

impl Drop for ProtectedControlSocket {
    fn drop(&mut self) {
        if self.directory.revalidate().is_ok()
            && socket_identity(&self.directory, &self.name, true).ok() == Some(self.identity)
        {
            let _ = self.directory.remove_child(&self.name);
        }
    }
}

impl ProtectedRuntimeDirectory {
    /// Callers supply a full SHA-256 basename. The lock is held through bind;
    /// parent and object identity are checked before cleanup and publication.
    pub async fn bind_control_socket(
        mut self,
        name: &OsStr,
    ) -> io::Result<(UnixListener, ProtectedControlSocket)> {
        self.require_private_leaf()?;
        let name = name
            .to_str()
            .filter(|name| {
                name.len() == 64
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "OHOS control socket requires a full lowercase SHA-256 basename",
                )
            })?;
        crate::ohos_runtime::check_socket_budget(self.path())?;
        let lock_name = OsString::from(format!("{name}.lock"));
        let lock = self.open_lock_file(&lock_name)?;
        let lock = tokio::task::spawn_blocking(move || {
            lock.lock()?;
            Ok::<_, io::Error>(lock)
        })
        .await
        .map_err(|error| io::Error::other(format!("socket lock task failed: {error}")))??;
        self.validate_lock_file(&lock_name, &lock)?;
        let name = OsString::from(name);
        let path = self.path().join(&name);
        match socket_identity(&self, &name, true) {
            Ok(stale) => {
                match UnixStream::connect(&path).await {
                    Ok(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            "OHOS control socket is already listening",
                        ));
                    }
                    Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {}
                    Err(error) => return Err(error),
                }
                self.revalidate()?;
                if socket_identity(&self, &name, true)? != stale {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "OHOS stale socket was replaced",
                    ));
                }
                self.remove_child(&name)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        self.revalidate()?;
        let listener = UnixListener::bind(&path).await.map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "OHOS control socket stage=bind path={}: {error}",
                    path.display()
                ),
            )
        })?;
        let identity = socket_identity(&self, &name, false)?;
        let guard = ProtectedControlSocket {
            directory: self,
            name,
            identity,
        };
        guard.directory.revalidate()?;
        let name_c = crate::ohos_runtime::component_name(&guard.name)?;
        // Only this newly bound, identity-checked object may be normalized.
        if unsafe { libc::fchmodat(guard.directory.as_raw_fd(), name_c.as_ptr(), 0o600, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        guard.directory.revalidate()?;
        if socket_identity(&guard.directory, &guard.name, true)? != identity {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "OHOS bound socket changed during publication",
            ));
        }
        drop(lock);
        Ok((listener, guard))
    }
}

fn socket_identity(
    directory: &ProtectedRuntimeDirectory,
    name: &OsStr,
    require_private_mode: bool,
) -> io::Result<SocketIdentity> {
    let metadata = directory.stat_child(name)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFSOCK
        || metadata.st_uid != unsafe { libc::geteuid() }
        || metadata.st_nlink != 1
        || (require_private_mode && metadata.st_mode & 0o7777 != 0o600)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "OHOS socket path={} owner={} mode={:04o}: require an owned socket with mode 0600 and one link; object preserved",
                directory.path().join(name).display(),
                metadata.st_uid,
                metadata.st_mode & 0o7777
            ),
        ));
    }
    Ok(SocketIdentity {
        device: metadata.st_dev as u64,
        inode: metadata.st_ino as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn fixture() -> tempfile::TempDir {
        #[cfg(target_os = "macos")]
        let root = "/private/tmp";
        #[cfg(not(target_os = "macos"))]
        let root = "/tmp";
        let temporary = tempfile::Builder::new()
            .prefix("cs")
            .tempdir_in(root)
            .unwrap();
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
        temporary
    }

    fn directory(path: &std::path::Path) -> ProtectedRuntimeDirectory {
        crate::validate_ohos_runtime_base(&fs::canonicalize(path).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn socket_roundtrip_concurrent_listener_refusal_and_owned_cleanup() {
        use tokio::io::AsyncReadExt;
        use tokio::io::AsyncWriteExt;
        let temporary = fixture();
        let name = "a".repeat(64);
        let (mut listener, guard) = directory(temporary.path())
            .bind_control_socket(OsStr::new(&name))
            .await
            .unwrap();
        assert_eq!(
            fs::metadata(guard.path()).unwrap().permissions().mode() & 0o7777,
            0o600
        );
        let second = directory(temporary.path())
            .bind_control_socket(OsStr::new(&name))
            .await;
        assert!(matches!(second, Err(error) if error.kind() == io::ErrorKind::AddrInUse));
        // The liveness probe above makes the first queued connection.
        drop(listener.accept().await.unwrap());
        let mut client = UnixStream::connect(guard.path()).await.unwrap();
        client.write_all(b"fixed-test-data").await.unwrap();
        let mut server = listener.accept().await.unwrap();
        let mut buffer = [0; 15];
        server.read_exact(&mut buffer).await.unwrap();
        assert_eq!(&buffer, b"fixed-test-data");
        let path = guard.path();
        drop(listener);
        drop(guard);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn stale_owned_socket_is_reused_but_foreign_objects_are_preserved() {
        let temporary = fixture();
        let name = "b".repeat(64);
        let path = temporary.path().join(&name);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        drop(listener);
        let (listener, guard) = directory(temporary.path())
            .bind_control_socket(OsStr::new(&name))
            .await
            .unwrap();
        drop(listener);
        drop(guard);
        fs::write(&path, b"keep").unwrap();
        assert!(
            directory(temporary.path())
                .bind_control_socket(OsStr::new(&name))
                .await
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"keep");
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("absent", &path).unwrap();
        assert!(
            directory(temporary.path())
                .bind_control_socket(OsStr::new(&name))
                .await
                .is_err()
        );
        assert_eq!(fs::read_link(path).unwrap(), PathBuf::from("absent"));
    }

    #[tokio::test]
    async fn cleanup_preserves_replacement_and_unsafe_socket_mode() {
        let temporary = fixture();
        let name = "c".repeat(64);
        let (listener, guard) = directory(temporary.path())
            .bind_control_socket(OsStr::new(&name))
            .await
            .unwrap();
        let path = guard.path();
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"replacement").unwrap();
        drop(guard);
        drop(listener);
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        fs::remove_file(&path).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        drop(listener);
        assert!(
            directory(temporary.path())
                .bind_control_socket(OsStr::new(&name))
                .await
                .is_err()
        );
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o7777,
            0o666
        );
    }
}
