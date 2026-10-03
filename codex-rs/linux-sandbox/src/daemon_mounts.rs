//! Find host mount aliases that need the privileged socket directory mask.
//! Mount roots describe filesystem identity; canonical paths alone miss bind mounts.
//! Btrfs subvolume device numbers can differ from the containing mount's device.

#[cfg(target_os = "linux")]
use rustix::fs::AtFlags;
#[cfg(target_os = "linux")]
use rustix::fs::StatxFlags;
#[cfg(target_os = "linux")]
use rustix::fs::fstatfs;
#[cfg(target_os = "linux")]
use rustix::fs::statx;
use std::collections::BTreeSet;
#[cfg(target_os = "linux")]
use std::fs;
use std::io;
use std::io::Read;
use std::io::Write;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt;
use std::os::unix::ffi::OsStringExt;
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
#[cfg(target_os = "linux")]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SocketFilesystem {
    Btrfs,
    Other,
}

const MOUNTINFO_LIMIT: usize = 1024 * 1024;

fn invalid(reason: &str, detail: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!(
        "cannot establish runtime mount isolation: reason={reason} {detail}"
    ))
}

fn escaped(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|byte| std::ascii::escape_default(*byte))
        .map(char::from)
        .collect()
}

struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

fn read_bounded(reader: impl Read, limit: usize) -> io::Result<Captured> {
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    let truncated = bytes.len() > limit;
    bytes.truncate(limit);
    Ok(Captured { bytes, truncated })
}

struct MountDiagnostics {
    enabled: bool,
    purpose: &'static str,
}

impl MountDiagnostics {
    fn event(&self, stage: &str, detail: impl std::fmt::Display) {
        if self.enabled {
            // Diagnostics must not turn a broken stderr pipe into a panic.
            let _ = writeln!(
                io::stderr().lock(),
                "[codex-mount] pid={} purpose={} stage={stage} {detail}",
                std::process::id(),
                self.purpose
            );
        }
    }

    #[cfg(target_os = "linux")]
    fn capture(&self, name: &str, result: &io::Result<Captured>) {
        if !self.enabled {
            return;
        }
        match result {
            Ok(capture) => {
                let _ = write_capture(&mut io::stderr().lock(), self.purpose, name, capture);
            }
            Err(error) => self.event(
                name,
                format_args!(
                    "status=error errno={:?} error=\"{}\"",
                    error.raw_os_error(),
                    escaped(error.to_string().as_bytes())
                ),
            ),
        }
    }
}

fn write_capture(
    writer: &mut impl Write,
    purpose: &str,
    name: &str,
    capture: &Captured,
) -> io::Result<()> {
    let prefix = format!("[codex-mount] pid={} purpose={purpose}", std::process::id());
    writeln!(
        writer,
        "{prefix} stage={name}-begin captured_bytes={} truncated={} encoding=ascii-escape",
        capture.bytes.len(),
        capture.truncated
    )?;
    for (index, chunk) in capture.bytes.chunks(1024).enumerate() {
        writeln!(
            writer,
            "{prefix} stage={name}-data offset={} data=\"{}\"",
            index * 1024,
            escaped(chunk)
        )?;
    }
    writeln!(
        writer,
        "{prefix} stage={name}-end captured_bytes={} truncated={}",
        capture.bytes.len(),
        capture.truncated
    )
}

fn fdinfo_mount_id(capture: &Captured) -> io::Result<Option<u64>> {
    if capture.truncated {
        return Err(invalid("fdinfo-truncated", "mount ID unavailable"));
    }
    let mut id = None;
    for value in capture
        .bytes
        .split(|byte| *byte == b'\n')
        .filter_map(|line| line.strip_prefix(b"mnt_id:"))
    {
        let parsed = std::str::from_utf8(value)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|id| *id != 0)
            .ok_or_else(|| invalid("fdinfo-mount-id-invalid", "expected a positive integer"))?;
        if id.replace(parsed).is_some() {
            return Err(invalid(
                "fdinfo-mount-id-duplicate",
                "multiple mnt_id fields",
            ));
        }
    }
    Ok(id)
}

fn collect_statx<T>(
    ohos: bool,
    diagnostics: bool,
    fd_mount_id: Option<u64>,
    query: impl FnOnce() -> T,
) -> Option<T> {
    // Preserve Linux's fdinfo fast path: a new syscall may be prohibited by
    // an existing container seccomp policy, even if its errno is ignored.
    (ohos || diagnostics || fd_mount_id.is_none()).then(query)
}

#[cfg(any(target_env = "ohos", test))]
#[derive(Clone, Copy)]
struct DirectoryIdentity {
    inode: u64,
    mode: u32,
    uid: u32,
    gid: u32,
}

#[cfg(any(target_env = "ohos", test))]
#[derive(Clone)]
struct StatxIdentity {
    inode: Option<u64>,
    mode: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
    mount_id: Option<u64>,
    device: String,
}

#[cfg(any(target_env = "ohos", test))]
fn ohos_mount_identity(
    metadata: DirectoryIdentity,
    fdinfo: io::Result<Option<u64>>,
    statx: io::Result<StatxIdentity>,
) -> io::Result<(String, u64)> {
    let stat = statx.map_err(|error| invalid("statx-unavailable", error))?;
    let (Some(inode), Some(mode), Some(uid), Some(gid), Some(mount_id)) =
        (stat.inode, stat.mode, stat.uid, stat.gid, stat.mount_id)
    else {
        return Err(invalid(
            "statx-fields-missing",
            "require TYPE MODE UID GID INO MNT_ID",
        ));
    };
    if metadata.mode & 0o170000 != 0o040000
        || inode != metadata.inode
        || mode != metadata.mode
        || uid != metadata.uid
        || gid != metadata.gid
    {
        return Err(invalid(
            "statx-identity-conflict",
            "inode/type/mode/uid/gid differ from the same FD metadata",
        ));
    }
    let fd_mount_id = fdinfo?.ok_or_else(|| {
        invalid(
            "fdinfo-mount-id-missing",
            "OHOS requires an independent mount ID",
        )
    })?;
    if mount_id == 0 || mount_id != fd_mount_id {
        return Err(invalid(
            "statx-mount-id-conflict",
            format_args!("fdinfo={fd_mount_id} statx={mount_id}"),
        ));
    }
    Ok((stat.device, mount_id))
}

#[cfg(any(target_env = "ohos", test))]
fn check_namespace(before: io::Result<PathBuf>, after: io::Result<PathBuf>) -> io::Result<()> {
    let before = before.map_err(|error| invalid("namespace-read", error))?;
    let after = after.map_err(|error| invalid("namespace-read", error))?;
    if before != after {
        return Err(invalid(
            "namespace-changed",
            "mount namespace changed during observation",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn daemon_socket_mask_paths(
    directory: &Path,
    masked_root: Option<&Path>,
    purpose: &'static str,
) -> io::Result<BTreeSet<PathBuf>> {
    let diagnostics = MountDiagnostics {
        enabled: std::env::var_os("CODEX_HARMONY_PROCESS_DIAGNOSTICS")
            .is_some_and(|value| value == "1"),
        purpose,
    };
    diagnostics.event(
        "begin",
        format_args!(
            "path=\"{}\" euid={} build=\"{}\"",
            escaped(directory.as_os_str().as_bytes()),
            unsafe { libc::geteuid() },
            escaped(
                option_env!("CODEX_HARMONY_BUILD_ID")
                    .unwrap_or("unknown")
                    .as_bytes()
            )
        ),
    );
    let result = inspect_directory_mounts(directory, masked_root, &diagnostics);
    match &result {
        Ok(paths) => diagnostics.event("accepted", format_args!("mask_paths={paths:?}")),
        Err(error) => diagnostics.event(
            "rejected",
            format_args!("error=\"{}\"", escaped(error.to_string().as_bytes())),
        ),
    }
    result.map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "mount isolation purpose={purpose} path=\"{}\": {error}",
                escaped(directory.as_os_str().as_bytes())
            ),
        )
    })
}

#[cfg(target_os = "linux")]
fn inspect_directory_mounts(
    directory: &Path,
    masked_root: Option<&Path>,
    diagnostics: &MountDiagnostics,
) -> io::Result<BTreeSet<PathBuf>> {
    let directory_file = fs::File::options()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory)
        .map_err(|error| invalid("open-directory", error))?;
    let metadata = directory_file
        .metadata()
        .map_err(|error| invalid("metadata", error))?;
    let device = format!(
        "{}:{}",
        libc::major(metadata.dev()),
        libc::minor(metadata.dev())
    );
    diagnostics.event(
        "metadata",
        format_args!(
            "fd={} dev={} dev_hex={:#x} decoded_device={device} ino={} uid={} gid={} mode={:#o}",
            directory_file.as_raw_fd(),
            metadata.dev(),
            metadata.dev(),
            metadata.ino(),
            metadata.uid(),
            metadata.gid(),
            metadata.mode()
        ),
    );
    let collect_namespace = cfg!(target_env = "ohos") || diagnostics.enabled;
    let namespace_before = collect_namespace.then(|| fs::read_link("/proc/self/ns/mnt"));
    diagnostics.event(
        "namespace-before",
        format_args!("result={namespace_before:?}"),
    );
    let fdinfo = fs::File::open(format!("/proc/self/fdinfo/{}", directory_file.as_raw_fd()))
        .and_then(|file| read_bounded(file, 64 * 1024));
    diagnostics.capture("fdinfo", &fdinfo);
    let fd_mount_id = fdinfo
        .as_ref()
        .map_err(|error| invalid("fdinfo-read", error))
        .and_then(fdinfo_mount_id);
    diagnostics.event("fdinfo-id", format_args!("result={fd_mount_id:?}"));
    let statx_mask = if cfg!(target_env = "ohos") || diagnostics.enabled {
        StatxFlags::BASIC_STATS | StatxFlags::MNT_ID
    } else {
        StatxFlags::MNT_ID
    };
    let stat = collect_statx(
        cfg!(target_env = "ohos"),
        diagnostics.enabled,
        fd_mount_id.as_ref().ok().copied().flatten(),
        || statx(&directory_file, "", AtFlags::EMPTY_PATH, statx_mask),
    );
    match &stat {
        Some(Ok(stat)) => diagnostics.event("statx", format_args!("status=ok flags={:#x} requested_mask={:#x} returned_mask={:#x} dev_major={} dev_minor={} ino={} mode={:#o} uid={} gid={} mount_id={} mount_id_valid={}", AtFlags::EMPTY_PATH.bits(), statx_mask.bits(), stat.stx_mask, stat.stx_dev_major, stat.stx_dev_minor, stat.stx_ino, stat.stx_mode, stat.stx_uid, stat.stx_gid, stat.stx_mnt_id, stat.stx_mask & StatxFlags::MNT_ID.bits() != 0)),
        Some(Err(error)) => diagnostics.event("statx", format_args!("status=error requested_mask={:#x} errno={} error={error}", statx_mask.bits(), error.raw_os_error())),
        None => diagnostics.event("statx", "status=not-requested source=fdinfo"),
    }
    let filesystem_stat = fstatfs(&directory_file);
    match &filesystem_stat {
        Ok(stat) => diagnostics.event("fstatfs", format_args!("type={:#x}", stat.f_type)),
        Err(error) => diagnostics.event(
            "fstatfs",
            format_args!("errno={} error={error}", error.raw_os_error()),
        ),
    }
    let mountinfo =
        fs::File::open("/proc/self/mountinfo").and_then(|file| read_bounded(file, MOUNTINFO_LIMIT));
    diagnostics.capture("mountinfo", &mountinfo);
    let namespace_after = collect_namespace.then(|| fs::read_link("/proc/self/ns/mnt"));
    diagnostics.event(
        "namespace-after",
        format_args!("result={namespace_after:?}"),
    );
    let mountinfo = mountinfo.map_err(|error| invalid("mountinfo-read", error))?;
    if mountinfo.truncated {
        return Err(invalid(
            "mountinfo-truncated",
            format_args!("limit={MOUNTINFO_LIMIT}"),
        ));
    }
    #[cfg(target_env = "ohos")]
    let (device, mount_id, filesystem) = {
        check_namespace(
            namespace_before.ok_or_else(|| {
                invalid(
                    "namespace-not-requested",
                    "OHOS requires namespace identity",
                )
            })?,
            namespace_after.ok_or_else(|| {
                invalid(
                    "namespace-not-requested",
                    "OHOS requires namespace identity",
                )
            })?,
        )?;
        let (device, mount_id) = ohos_mount_identity(
            DirectoryIdentity {
                inode: metadata.ino(),
                mode: metadata.mode(),
                uid: metadata.uid(),
                gid: metadata.gid(),
            },
            fd_mount_id,
            stat.ok_or_else(|| invalid("statx-not-requested", "OHOS requires statx identity"))?
                .map(|stat| {
                    let has = |flags: StatxFlags| stat.stx_mask & flags.bits() == flags.bits();
                    StatxIdentity {
                        inode: has(StatxFlags::INO).then_some(stat.stx_ino),
                        mode: has(StatxFlags::TYPE | StatxFlags::MODE)
                            .then_some(u32::from(stat.stx_mode)),
                        uid: has(StatxFlags::UID).then_some(stat.stx_uid),
                        gid: has(StatxFlags::GID).then_some(stat.stx_gid),
                        mount_id: has(StatxFlags::MNT_ID).then_some(stat.stx_mnt_id),
                        device: format!("{}:{}", stat.stx_dev_major, stat.stx_dev_minor),
                    }
                })
                .map_err(io::Error::from),
        )?;
        // OHOS uses the explicit, cross-checked statx identity, not a filesystem
        // name exception. The selected mountinfo device must match exactly.
        diagnostics.event(
            "identity",
            format_args!("source=statx-verified device={device} mount_id={mount_id}"),
        );
        (device, Some(mount_id.to_string()), SocketFilesystem::Other)
    };
    #[cfg(not(target_env = "ohos"))]
    let (device, mount_id, filesystem) = {
        let mount_id = fd_mount_id
            .ok()
            .flatten()
            .or_else(|| {
                stat.and_then(Result::ok)
                    .filter(|stat| stat.stx_mask & StatxFlags::MNT_ID.bits() != 0)
                    .map(|stat| stat.stx_mnt_id)
            })
            .map(|id| id.to_string());
        let filesystem = if filesystem_stat.is_ok_and(|stat| stat.f_type == libc::BTRFS_SUPER_MAGIC)
        {
            SocketFilesystem::Btrfs
        } else {
            SocketFilesystem::Other
        };
        (device, mount_id, filesystem)
    };
    check_mounts_observed(
        directory,
        &device,
        filesystem,
        mount_id.as_deref(),
        &mountinfo.bytes,
        masked_root,
        diagnostics,
    )
}

#[cfg(test)]
fn check_mounts(
    directory: &Path,
    device: &str,
    filesystem: SocketFilesystem,
    mount_id: Option<&str>,
    mountinfo: &[u8],
    masked_root: Option<&Path>,
) -> io::Result<BTreeSet<PathBuf>> {
    check_mounts_observed(
        directory,
        device,
        filesystem,
        mount_id,
        mountinfo,
        masked_root,
        &MountDiagnostics {
            enabled: false,
            purpose: "test",
        },
    )
}

fn check_mounts_observed(
    directory: &Path,
    device: &str,
    filesystem: SocketFilesystem,
    mount_id: Option<&str>,
    mountinfo: &[u8],
    masked_root: Option<&Path>,
    diagnostics: &MountDiagnostics,
) -> io::Result<BTreeSet<PathBuf>> {
    let mut mounts = Vec::new();
    let mut ids = BTreeSet::new();
    for (line_number, line) in mountinfo
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .enumerate()
    {
        let fields: Vec<_> = line.split(|byte| *byte == b' ').collect();
        let separator = fields.iter().position(|field| *field == b"-");
        let parse_error = || invalid("mountinfo-parse", format_args!("line={}", line_number + 1));
        let Some(separator) = separator.filter(|index| *index >= 6 && fields.len() == *index + 4)
        else {
            return Err(parse_error());
        };
        if fields.iter().any(|field| field.is_empty()) {
            return Err(parse_error());
        }
        let [id, parent, mount_device, root, destination] = &fields[..5] else {
            unreachable!()
        };
        let number = |bytes: &[u8]| {
            std::str::from_utf8(bytes)
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|value| value.to_string().as_bytes() == bytes)
        };
        if number(id).is_none_or(|id| id == 0)
            || number(parent).is_none()
            || mount_device.split(|byte| *byte == b':').count() != 2
            || mount_device
                .split(|byte| *byte == b':')
                .any(|part| number(part).is_none_or(|value| value > u64::from(u32::MAX)))
        {
            return Err(parse_error());
        }
        if !ids.insert(*id) {
            return Err(invalid(
                "mount-id-duplicate",
                format_args!("id={}", escaped(id)),
            ));
        }
        let destination = mount_path(destination).map_err(|error| {
            invalid(
                "mount-destination-invalid",
                format_args!("line={} {error}", line_number + 1),
            )
        })?;
        let mount_filesystem = fields[separator + 1];
        mounts.push((
            *id,
            *parent,
            *mount_device,
            *root,
            destination,
            mount_filesystem,
        ));
    }
    let selected_index = mount_id
        .map(|mount_id| {
            let mut matching = mounts
                .iter()
                .enumerate()
                .filter(|(_, (id, ..))| *id == mount_id.as_bytes());
            let (index, (id, parent, mount_device, root, destination, mount_filesystem)) =
                matching.next().ok_or_else(|| invalid("mount-id-not-found", format_args!("id={mount_id}")))?;
            diagnostics.event("selected-mount", format_args!("id={} parent={} device={} root=\"{}\" destination={destination:?} filesystem=\"{}\"", escaped(id), escaped(parent), escaped(mount_device), escaped(root), escaped(mount_filesystem)));
            if *mount_device != device.as_bytes()
                && (filesystem != SocketFilesystem::Btrfs || *mount_filesystem != b"btrfs") {
                return Err(invalid("device-mismatch", format_args!("id={mount_id} expected={device} observed={} filesystem=\"{}\"", escaped(mount_device), escaped(mount_filesystem))));
            }
            Ok(index)
        })
        .transpose()?;
    // Btrfs reports a per-subvolume st_dev, while mountinfo uses the superblock
    // device. Only the verified descriptor's mount ID can authorize that mismatch.
    // Retain both device identities when looking for aliases.
    let mount_device = selected_index
        .map(|index| mounts[index].2)
        .unwrap_or(device.as_bytes());
    let mounts = mounts
        .into_iter()
        .map(|(id, parent, candidate_device, root, destination, _)| {
            // Only roots on the socket filesystem can identify aliases. Other
            // filesystems can use non-path roots such as nsfs `mnt:[inode]`, but
            // their destinations still matter for ancestry and nested-mount checks.
            let root = (candidate_device == device.as_bytes() || candidate_device == mount_device)
                .then(|| mount_path(root))
                .transpose()
                .map_err(|error| {
                    invalid(
                        "mount-root-invalid",
                        format_args!("id={} {error}", escaped(id)),
                    )
                })?;
            Ok((id, parent, candidate_device, root, destination))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let (location, containing_mount) = if let Some((mount_id, index)) = mount_id.zip(selected_index)
    {
        // fdinfo/statx identifies the opened mount, which may have been covered
        // by another mount before we read mountinfo.
        let selected = &mounts[index];
        let (_, _, _, root, destination) = selected;
        let root = root
            .as_ref()
            .ok_or_else(|| invalid("selected-root-missing", format_args!("id={mount_id}")))?;
        let relative = directory.strip_prefix(destination).map_err(|_| {
            invalid(
                "path-outside-mount",
                format_args!("id={mount_id} destination={destination:?}"),
            )
        })?;
        let mut current = Some(selected);
        let mut visible_child: Option<&Path> = None;
        let mut visited = BTreeSet::new();
        while let Some((id, parent, _, _, destination)) = current {
            if !visited.insert(id) {
                return Err(invalid(
                    "mount-parent-cycle",
                    format_args!("id={}", escaped(id)),
                ));
            }
            if mounts.iter().any(|(child_id, child_parent, _, _, child)| {
                child_id != id
                    && child_parent == id
                    && directory.starts_with(child)
                    && !visible_child.is_some_and(|visible| child.starts_with(visible))
            }) {
                return Err(invalid("mount-covered", format_args!("id={}", escaped(id))));
            }
            if id == parent {
                break;
            }
            // Follow the selected branch towards the namespace root. Sibling
            // mounts below this branch are hidden; mounts above it cover it.
            visible_child = Some(destination);
            current = mounts.iter().find(|(id, ..)| id == parent);
        }
        (root.join(relative), Some((mount_id, destination)))
    } else {
        // Without a mount ID, require every possible containing mount to agree
        // on the backing location, and do not assume any aliases are hidden.
        let locations: BTreeSet<_> = mounts
            .iter()
            .filter_map(|(_, _, _, root, destination)| {
                let root = root.as_ref()?;
                directory
                    .strip_prefix(destination)
                    .ok()
                    .map(|relative| root.join(relative))
            })
            .collect();
        if locations.len() != 1 {
            return Err(invalid(
                "mount-location-ambiguous",
                format_args!("candidate_locations={}", locations.len()),
            ));
        }
        (
            locations
                .into_iter()
                .next()
                .ok_or_else(|| invalid("mount-location-missing", "no containing mount"))?,
            None,
        )
    };
    let mut mask_paths = BTreeSet::from([directory.to_path_buf()]);
    for (id, _, _, root, destination) in &mounts {
        // Nested mounts can introduce another filesystem (or an individual socket) under the mask.
        let nested = destination != directory && destination.starts_with(directory);
        let alias = if let Some(root) = root {
            if let Ok(relative) = location.strip_prefix(root) {
                Some((destination.join(relative), !relative.as_os_str().is_empty()))
            } else if root.starts_with(&location) {
                Some((destination.clone(), false))
            } else {
                None
            }
        } else {
            None
        };
        let exposed_alias = alias.filter(|(path, _)| {
            // An ancestor's path beneath this mount is hidden by it. Keep
            // checking other mounts, including aliases mounted beneath it.
            let hidden = containing_mount.is_some_and(|(mount_id, containing_mount)| {
                *id != mount_id.as_bytes()
                    && containing_mount.starts_with(destination)
                    && path.starts_with(containing_mount)
            });
            !path.starts_with(directory)
                && !masked_root.is_some_and(|root| path.starts_with(root))
                && !hidden
        });
        if nested
            || exposed_alias
                .as_ref()
                .is_some_and(|(_, can_mask)| !can_mask)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "runtime directory has an unsupported host mount: reason={} id={} destination={destination:?}; remove the bind-mount alias or nested mount before starting the sandbox",
                    if nested {
                        "nested-mount"
                    } else {
                        "direct-bind-alias"
                    },
                    escaped(id)
                ),
            ));
        }
        if let Some((path, _)) = exposed_alias {
            // An alias of an ancestor exposes the whole directory, so apply the
            // same mask there. Direct directory/socket aliases and nested mounts
            // remain unsupported because a later bind could reopen the mask.
            mask_paths.insert(path);
        }
    }
    Ok(mask_paths)
}

fn mount_path(encoded: &[u8]) -> io::Result<PathBuf> {
    let mut decoded = Vec::new();
    let mut bytes = encoded.iter().copied();
    while let Some(byte) = bytes.next() {
        decoded.push(if byte == b'\\' {
            let digits: Vec<_> = bytes.by_ref().take(3).collect();
            match digits.as_slice() {
                b"040" => b' ',
                b"011" => b'\t',
                b"012" => b'\n',
                b"134" => b'\\',
                _ => {
                    return Err(invalid(
                        "path-escape-invalid",
                        "invalid mountinfo octal escape",
                    ));
                }
            }
        } else {
            byte
        });
    }
    let path = PathBuf::from(std::ffi::OsString::from_vec(decoded));
    if !path.is_absolute() {
        return Err(invalid(
            "path-not-absolute",
            "mountinfo path is not absolute",
        ));
    }
    Ok(path)
}

#[cfg(test)]
#[path = "daemon_mounts_tests.rs"]
mod tests;
