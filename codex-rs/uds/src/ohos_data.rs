//! Shared CLI initialization. Never infer a safe directory from HOME or a UID.

use super::OhosRuntimePurpose;
use super::ProtectedRuntimeDirectory;
use super::RuntimeProfile;
#[cfg(any(target_env = "ohos", test))]
use super::check_socket_budget;
#[cfg(any(target_env = "ohos", test))]
use super::open_base_with_profile;
#[cfg(any(target_env = "ohos", test))]
use super::validate_metadata;
#[cfg(any(target_env = "ohos", test))]
use std::ffi::OsStr;
use std::io;
use std::path::Path;
#[cfg(any(target_env = "ohos", test))]
use std::path::PathBuf;
#[cfg(target_env = "ohos")]
use std::sync::Mutex;
#[cfg(target_env = "ohos")]
use std::sync::OnceLock;

#[cfg(any(target_env = "ohos", test))]
#[path = "ohos_context.rs"]
mod context;

const PLATFORM_FILES: &str = "/data/storage/el2/base/files";

/// A platform namespace candidate, never an environment-selected directory.
/// Its presence does not prove that HiShell has an application data namespace.
pub fn ohos_platform_files_candidate() -> &'static str {
    PLATFORM_FILES
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OhosProcessIdentity {
    pub uid: libc::uid_t,
    pub euid: libc::uid_t,
    pub gid: libc::gid_t,
    pub egid: libc::gid_t,
}

impl OhosProcessIdentity {
    pub fn current() -> Self {
        // GID and UID are separate kernel facts; no equality is required.
        Self {
            uid: unsafe { libc::getuid() },
            euid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getgid() },
            egid: unsafe { libc::getegid() },
        }
    }

    fn validate_current(self, current: Self) -> io::Result<()> {
        if self != current {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "OHOS data-dir stage=identity-changed: initialized={self:?} current={current:?}; restart under a stable identity"
                ),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OhosDirectorySource {
    ApplicationContext,
    ValidatedPlatformNamespace,
}

/// Pins every verified chain for the lifetime of the process. Successful
/// initialization is shared; failures are retryable and never repair modes.
#[derive(Debug)]
pub struct OhosDataDirectories {
    files: ProtectedRuntimeDirectory,
    root: ProtectedRuntimeDirectory,
    state: ProtectedRuntimeDirectory,
    runtime: ProtectedRuntimeDirectory,
    aliases: ProtectedRuntimeDirectory,
    sockets: ProtectedRuntimeDirectory,
    temporary: ProtectedRuntimeDirectory,
    logs: ProtectedRuntimeDirectory,
    identity: OhosProcessIdentity,
    source: OhosDirectorySource,
    context_unavailable: Option<String>,
}

impl OhosDataDirectories {
    pub fn files_dir(&self) -> &Path {
        self.files.path()
    }

    pub fn root_dir(&self) -> &Path {
        self.root.path()
    }

    pub fn state_dir(&self) -> &Path {
        self.state.path()
    }

    pub fn runtime_dir(&self) -> &Path {
        self.runtime.path()
    }

    pub fn temp_dir(&self) -> &Path {
        self.temporary.path()
    }

    pub fn logs_dir(&self) -> &Path {
        self.logs.path()
    }

    pub fn identity(&self) -> OhosProcessIdentity {
        self.identity
    }

    pub fn source(&self) -> OhosDirectorySource {
        self.source
    }

    pub fn context_unavailable_reason(&self) -> Option<&str> {
        self.context_unavailable.as_deref()
    }

    pub fn revalidate(&self) -> io::Result<()> {
        self.identity
            .validate_current(OhosProcessIdentity::current())?;
        for directory in [
            &self.files,
            &self.root,
            &self.state,
            &self.runtime,
            &self.aliases,
            &self.sockets,
            &self.temporary,
            &self.logs,
        ] {
            directory.revalidate()?;
        }
        Ok(())
    }

    pub(super) fn prepare_runtime_directory(
        &self,
        purpose: OhosRuntimePurpose,
    ) -> io::Result<ProtectedRuntimeDirectory> {
        self.revalidate()?;
        let directory = match purpose {
            OhosRuntimePurpose::Aliases => &self.aliases,
            OhosRuntimePurpose::ControlSockets => &self.sockets,
        };
        let guard = ProtectedRuntimeDirectory {
            directories: directory.clone_directories()?,
            uid: self.identity.euid,
            profile: RuntimeProfile::Strict,
        };
        guard.revalidate()?;
        Ok(guard)
    }
}

#[cfg(target_env = "ohos")]
static DATA_DIRECTORIES: OnceLock<OhosDataDirectories> = OnceLock::new();
#[cfg(target_env = "ohos")]
static INITIALIZATION: Mutex<()> = Mutex::new(());

/// Must run before main-process dotenv/config, aliases, sockets and threads.
/// No GUI marker or inherited directory environment participates in selection.
#[cfg(target_env = "ohos")]
pub fn initialize_ohos_data_directories() -> io::Result<&'static OhosDataDirectories> {
    if let Some(directories) = DATA_DIRECTORIES.get() {
        directories.revalidate()?;
        return Ok(directories);
    }
    let _initialization = INITIALIZATION.lock().map_err(|_| {
        io::Error::other("OHOS data-dir stage=initialization: initialization lock poisoned")
    })?;
    if DATA_DIRECTORIES.get().is_none() {
        if super::ohos_runtime_profile_contract() != "platform"
            || super::ohos_runtime_base_contract().is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "OHOS data-dir stage=contract: platform initialization requires profile=platform and no compiled runtime-base override",
            ));
        }
        let identity = OhosProcessIdentity::current();
        let (files, source, unavailable) = select_files_root(context::query_files_dir()?)?;
        let context_details = unavailable
            .clone()
            .unwrap_or_else(|| "available".to_owned());
        let directories = initialize_at(&files, identity, source, unavailable).map_err(|error| {
            io::Error::new(error.kind(), format!("OHOS platform data unavailable: uid={} euid={} gid={} egid={} source={source:?} context={context_details}; {error}; a protected application files namespace is required; HiShell support is not established by Context API availability alone", identity.uid, identity.euid, identity.gid, identity.egid))
        })?;
        DATA_DIRECTORIES.set(directories).map_err(|_| {
            io::Error::other("OHOS data-dir stage=initialization: inconsistent initialization")
        })?;
    }
    let directories = DATA_DIRECTORIES.get().expect("initialized under lock");
    directories.revalidate()?;
    Ok(directories)
}

#[cfg(not(target_env = "ohos"))]
pub fn initialize_ohos_data_directories() -> io::Result<&'static OhosDataDirectories> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "OHOS data-dir stage=platform: native initialization is available only on OHOS",
    ))
}

#[cfg(any(target_env = "ohos", test))]
fn select_files_root(
    result: context::ContextFiles,
) -> io::Result<(PathBuf, OhosDirectorySource, Option<String>)> {
    match result {
        context::ContextFiles::Available(path) => {
            context::validate_context_path(&path)?;
            Ok((path, OhosDirectorySource::ApplicationContext, None))
        }
        context::ContextFiles::Unavailable(reason) => Ok((
            PathBuf::from(PLATFORM_FILES),
            OhosDirectorySource::ValidatedPlatformNamespace,
            Some(reason),
        )),
    }
}

#[cfg(any(target_env = "ohos", test))]
fn initialize_at(
    files: &Path,
    identity: OhosProcessIdentity,
    source: OhosDirectorySource,
    context_unavailable: Option<String>,
) -> io::Result<OhosDataDirectories> {
    // Check the full address before creating even the data root.
    check_socket_budget(&files.join("codex/r/s"))?;
    let mut files = open_base_with_profile(files, identity.euid, RuntimeProfile::Strict)?;
    validate_metadata(
        files.path(),
        &files.leaf().file.metadata()?,
        identity.euid,
        true,
    )?;
    files.directories.last_mut().expect("files leaf").private = true;
    files.revalidate()?;
    let root = files.ensure_private_subdirectory(OsStr::new("codex"))?;
    let state = root.ensure_private_subdirectory(OsStr::new("state"))?;
    let runtime = root.ensure_private_subdirectory(OsStr::new("r"))?;
    let aliases = runtime.ensure_private_subdirectory(OsStr::new("a"))?;
    let sockets = runtime.ensure_private_subdirectory(OsStr::new("s"))?;
    let temporary = root.ensure_private_subdirectory(OsStr::new("tmp"))?;
    let logs = root.ensure_private_subdirectory(OsStr::new("logs"))?;
    let directories = OhosDataDirectories {
        files,
        root,
        state,
        runtime,
        aliases,
        sockets,
        temporary,
        logs,
        identity,
        source,
        context_unavailable,
    };
    directories.revalidate()?;
    Ok(directories)
}

#[cfg(test)]
#[path = "ohos_data_tests.rs"]
mod tests;
