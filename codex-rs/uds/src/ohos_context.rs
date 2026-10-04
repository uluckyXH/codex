//! Optional native Context query. Load only fixed, protected system libraries;
//! neither LD_LIBRARY_PATH nor an inherited tool variable selects the source.

use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::path::PathBuf;

pub(super) enum ContextFiles {
    Available(PathBuf),
    Unavailable(String),
}

pub(super) fn validate_context_path(path: &Path) -> io::Result<()> {
    let bytes = path.as_os_str().as_bytes();
    if !path.is_absolute()
        || bytes.len() <= 1
        || bytes[1..]
            .split(|byte| *byte == b'/')
            .any(|part| part.is_empty() || part == b"." || part == b"..")
        || bytes.iter().any(|byte| *byte < 32 || *byte == 127)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "OHOS data-dir stage=context-path: Context returned an invalid absolute files directory; no other root will be tried",
        ));
    }
    Ok(())
}

fn decode_context_result(code: i32, length: i32, bytes: &[u8]) -> io::Result<ContextFiles> {
    match code {
        16000011 => {
            return Ok(ContextFiles::Unavailable(
                "context_not_exist (16000011)".into(),
            ));
        }
        801 => {
            return Ok(ContextFiles::Unavailable(
                "context_not_supported (801)".into(),
            ));
        }
        0 => {}
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "OHOS data-dir stage=context-query: ApplicationContextGetFilesDir returned {code}; no alternate root will be tried"
                ),
            ));
        }
    }
    let length = usize::try_from(length).ok().filter(|length| {
        *length > 0 && *length < bytes.len() && bytes[*length] == 0 && !bytes[..*length].contains(&0)
    }).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "OHOS data-dir stage=context-query: invalid Context path length or terminator; no alternate root will be tried"))?;
    let path = std::str::from_utf8(&bytes[..length]).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "OHOS data-dir stage=context-query: files directory is not UTF-8",
        )
    })?;
    let path = PathBuf::from(path);
    validate_context_path(&path)?;
    Ok(ContextFiles::Available(path))
}

#[cfg(target_env = "ohos")]
pub(super) fn query_files_dir() -> io::Result<ContextFiles> {
    use super::super::RuntimeProfile;
    use super::super::open_base_with_profile;
    use std::ffi::CString;
    use std::fs::File;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::OpenOptionsExt;

    type ContextQuery = unsafe extern "C" fn(*mut libc::c_char, i32, *mut i32) -> i32;
    let mut unavailable = Vec::new();
    // These are system-library locations, not a search path supplied by the
    // invoking shell. Unsupported platforms can still validate PLATFORM_FILES.
    for name in [
        "/system/lib64/platformsdk/libability_runtime.so",
        "/system/lib64/libability_runtime.so",
    ] {
        let path = Path::new(name);
        let library = (|| -> io::Result<(File, super::super::ProtectedRuntimeDirectory)> {
            let parent = open_base_with_profile(
                path.parent().expect("system library parent"),
                unsafe { libc::geteuid() },
                RuntimeProfile::Strict,
            )?;
            let library = File::options()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)?;
            let metadata = library.metadata()?;
            if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Context library is not a protected root-owned regular file",
                ));
            }
            parent.revalidate()?;
            Ok((library, parent))
        })();
        let (_library, parent) = match library {
            Ok(library) => library,
            Err(error) => {
                unavailable.push(format!("{name}: {error}"));
                continue;
            }
        };
        let name_c = CString::new(name).expect("fixed library name has no NUL");
        let handle = unsafe { libc::dlopen(name_c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            unavailable.push(format!("{name}: native library unavailable"));
            continue;
        }
        // Keep the native library reference until process exit. Unloading
        // AbilityRuntime while it retains platform state is not supported.
        parent.revalidate()?;
        let symbol = unsafe {
            libc::dlsym(
                handle,
                c"OH_AbilityRuntime_ApplicationContextGetFilesDir".as_ptr(),
            )
        };
        if symbol.is_null() {
            unavailable.push(format!("{name}: Context files symbol unavailable"));
            continue;
        }
        let query: ContextQuery = unsafe { std::mem::transmute(symbol) };
        let mut bytes = [0xff_u8; 4096];
        let mut length = -1;
        let code = unsafe { query(bytes.as_mut_ptr().cast(), bytes.len() as i32, &mut length) };
        return decode_context_result(code, length, &bytes);
    }
    Ok(ContextFiles::Unavailable(unavailable.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_unavailability_is_distinct_from_an_invalid_response() {
        assert!(matches!(
            decode_context_result(16000011, -1, &[]).unwrap(),
            ContextFiles::Unavailable(_)
        ));
        assert!(matches!(
            decode_context_result(801, -1, &[]).unwrap(),
            ContextFiles::Unavailable(_)
        ));
        assert!(decode_context_result(401, -1, &[]).is_err());
        assert!(decode_context_result(16000050, -1, &[]).is_err());
        assert!(decode_context_result(0, -1, b"/data\0").is_err());
        assert!(decode_context_result(0, 6, b"/data\0").is_err());
        assert!(decode_context_result(0, 5, b"/data!").is_err());
        assert!(decode_context_result(0, 5, b"/d\0ta\0").is_err());
        assert!(decode_context_result(0, 2, b"/\xff\0").is_err());
    }

    #[test]
    fn context_path_must_be_absolute_and_unambiguous() {
        for path in [
            "", "/", "relative", "/a//b", "/a/", "/a/../b", "/a/./b", "/a\nb", "/a\0b",
        ] {
            assert!(validate_context_path(Path::new(path)).is_err(), "{path:?}");
        }
        match decode_context_result(0, 5, b"/data\0").unwrap() {
            ContextFiles::Available(path) => assert_eq!(path, Path::new("/data")),
            ContextFiles::Unavailable(_) => panic!("valid native result"),
        }
    }
}
