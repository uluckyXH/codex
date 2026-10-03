use super::*;
use pretty_assertions::assert_eq;
use test_case::test_case;

#[test]
fn linux_fdinfo_fast_path_does_not_issue_statx_but_ohos_always_does() {
    let result: Option<()> = collect_statx(false, false, Some(3996), || {
        panic!("statx must not run on the quiet Linux fdinfo fast path")
    });
    assert_eq!(result, None);
    for (ohos, diagnostics, id) in [
        (false, false, None),
        (false, true, Some(3996)),
        (true, false, Some(3996)),
        (true, false, None),
    ] {
        let calls = std::cell::Cell::new(0);
        let result = collect_statx(ohos, diagnostics, id, || {
            calls.set(calls.get() + 1);
            Err::<(), _>(io::ErrorKind::Unsupported)
        });
        assert_eq!(calls.get(), 1);
        assert_eq!(result, Some(Err(io::ErrorKind::Unsupported)));
    }
}

// Synthetic data informed by the summary report, not a replay of a device
// mount table. No original Rust-process mountinfo or statx dump was received.
fn synthetic_ohos_identity() -> (DirectoryIdentity, StatxIdentity) {
    (
        DirectoryIdentity {
            inode: 362362,
            mode: 0o40700,
            uid: 20020101,
            gid: 20020101,
        },
        StatxIdentity {
            inode: Some(362362),
            mode: Some(0o40700),
            uid: Some(20020101),
            gid: Some(20020101),
            mount_id: Some(3996),
            device: "0:66699".to_owned(),
        },
    )
}

const SYNTHETIC_OHOS_DIRECTORY: &str = "/data/storage/el2/base/files/c01317b85/a";
const SYNTHETIC_OHOS_MOUNTS: &str = "1 0 8:1 / / rw - ext4 system rw\n3996 1 0:66699 / /data/storage/el2/base rw - hmfs storage rw\n";

#[test]
fn synthetic_ohos_statx_identity_resolves_device_encoding_without_losing_aliases() {
    let directory = Path::new(SYNTHETIC_OHOS_DIRECTORY);
    let legacy = check_mounts(
        directory,
        "260:139",
        Some("3996"),
        SYNTHETIC_OHOS_MOUNTS.as_bytes(),
    )
    .unwrap_err();
    assert!(legacy.to_string().contains("reason=device-mismatch"));
    assert!(
        legacy
            .to_string()
            .contains("expected=260:139 observed=0:66699")
    );
    let (metadata, statx) = synthetic_ohos_identity();
    let (device, mount_id) = ohos_mount_identity(metadata, Ok(Some(3996)), Ok(statx)).unwrap();
    let mounts =
        format!("{SYNTHETIC_OHOS_MOUNTS}5000 1 0:66699 / /storage-alias rw - hmfs storage rw\n");
    assert_eq!(
        check_mounts(
            directory,
            &device,
            Some(&mount_id.to_string()),
            mounts.as_bytes()
        )
        .unwrap(),
        BTreeSet::from([
            directory.to_path_buf(),
            PathBuf::from("/storage-alias/files/c01317b85/a")
        ])
    );
    // The alternative source does not authorize a second device mismatch.
    let conflict = mounts.replace("0:66699", "0:66700");
    assert!(
        check_mounts(directory, &device, Some("3996"), conflict.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("reason=device-mismatch")
    );
}

#[test_case("inode"; "missing inode")]
#[test_case("mode"; "missing type or mode")]
#[test_case("uid"; "missing uid")]
#[test_case("gid"; "missing gid")]
#[test_case("mount"; "missing mount id")]
fn ohos_statx_missing_fields_cannot_authorize_identity(field: &str) {
    let (metadata, mut statx) = synthetic_ohos_identity();
    match field {
        "inode" => statx.inode = None,
        "mode" => statx.mode = None,
        "uid" => statx.uid = None,
        "gid" => statx.gid = None,
        "mount" => statx.mount_id = None,
        _ => unreachable!(),
    }
    assert!(
        ohos_mount_identity(metadata, Ok(Some(3996)), Ok(statx))
            .unwrap_err()
            .to_string()
            .contains("reason=statx-fields-missing")
    );
}

#[test_case("inode"; "inode conflict")]
#[test_case("mode"; "mode conflict")]
#[test_case("uid"; "uid conflict")]
#[test_case("gid"; "gid conflict")]
#[test_case("type"; "not a directory")]
fn ohos_statx_conflicting_identity_is_rejected(field: &str) {
    let (mut metadata, mut statx) = synthetic_ohos_identity();
    match field {
        "inode" => statx.inode = Some(1),
        "mode" => statx.mode = Some(0o40750),
        "uid" => statx.uid = Some(1),
        "gid" => statx.gid = Some(1),
        "type" => {
            metadata.mode = 0o100700;
            statx.mode = Some(metadata.mode);
        }
        _ => unreachable!(),
    }
    assert!(
        ohos_mount_identity(metadata, Ok(Some(3996)), Ok(statx))
            .unwrap_err()
            .to_string()
            .contains("reason=statx-identity-conflict")
    );
}

#[test]
fn ohos_identity_requires_both_mount_id_sources_and_available_statx() {
    let (metadata, statx) = synthetic_ohos_identity();
    for id in [0, 3997] {
        let mut conflicting = statx.clone();
        conflicting.mount_id = Some(id);
        assert!(
            ohos_mount_identity(metadata, Ok(Some(3996)), Ok(conflicting))
                .unwrap_err()
                .to_string()
                .contains("reason=statx-mount-id-conflict")
        );
    }
    assert!(
        ohos_mount_identity(metadata, Ok(None), Ok(statx.clone()))
            .unwrap_err()
            .to_string()
            .contains("reason=fdinfo-mount-id-missing")
    );
    assert!(
        ohos_mount_identity(metadata, Err(invalid("fdinfo-read", "denied")), Ok(statx))
            .unwrap_err()
            .to_string()
            .contains("reason=fdinfo-read")
    );
    assert!(
        ohos_mount_identity(
            metadata,
            Ok(Some(3996)),
            Err(io::Error::from_raw_os_error(38))
        )
        .unwrap_err()
        .to_string()
        .contains("reason=statx-unavailable")
    );
}

#[test_case("5000 1 0:66699 /files/c01317b85/a /direct rw - hmfs storage rw\n", "direct-bind-alias"; "directory alias")]
#[test_case("5000 1 0:66699 /files/c01317b85/a/rpc /socket rw - hmfs storage rw\n", "direct-bind-alias"; "socket alias")]
#[test_case("5000 3996 0:5 / /data/storage/el2/base/files/c01317b85/a/child rw - tmpfs tmpfs rw\n", "nested-mount"; "nested other filesystem")]
#[test_case("5000 3996 0:66699 /other /data/storage/el2/base rw - hmfs storage rw\n", "mount-covered"; "covered selected mount")]
#[test_case("3996 1 0:66699 / /duplicate rw - hmfs storage rw\n", "mount-id-duplicate"; "duplicate selected id")]
fn synthetic_ohos_identity_retains_mount_rejections(extra: &str, reason: &str) {
    let (metadata, statx) = synthetic_ohos_identity();
    let (device, mount_id) = ohos_mount_identity(metadata, Ok(Some(3996)), Ok(statx)).unwrap();
    let mounts = format!("{SYNTHETIC_OHOS_MOUNTS}{extra}");
    let error = check_mounts(
        Path::new(SYNTHETIC_OHOS_DIRECTORY),
        &device,
        Some(&mount_id.to_string()),
        mounts.as_bytes(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains(&format!("reason={reason}")),
        "{error}"
    );
}

#[test]
fn malformed_tables_and_unrelated_duplicate_ids_have_specific_rejections() {
    let directory = Path::new(SYNTHETIC_OHOS_DIRECTORY);
    for malformed in [
        "1 0 0:1 / /",
        "1 0 0:1 / / rw - hmfs",
        "bad 0 0:1 / / rw - hmfs dev rw",
        "1 0 nope / / rw - hmfs dev rw",
    ] {
        assert!(
            check_mounts(directory, "0:1", Some("1"), malformed.as_bytes())
                .unwrap_err()
                .to_string()
                .contains("reason=mountinfo-parse")
        );
    }
    let duplicate = format!(
        "{SYNTHETIC_OHOS_MOUNTS}2 1 0:8 / /other rw - tmpfs tmpfs rw\n2 1 0:9 / /unrelated rw - tmpfs tmpfs rw\n"
    );
    assert!(
        check_mounts(directory, "0:66699", Some("3996"), duplicate.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("reason=mount-id-duplicate")
    );
    assert!(
        check_mounts(
            directory,
            "0:66699",
            Some("3997"),
            SYNTHETIC_OHOS_MOUNTS.as_bytes()
        )
        .unwrap_err()
        .to_string()
        .contains("reason=mount-id-not-found")
    );
}

#[test]
fn fdinfo_requires_one_complete_positive_mount_id() {
    for bytes in [
        b"mnt_id:\t0\n".as_slice(),
        b"mnt_id: bad\n",
        b"mnt_id: 1\nmnt_id: 1\n",
    ] {
        assert!(
            fdinfo_mount_id(&Captured {
                bytes: bytes.to_vec(),
                truncated: false
            })
            .is_err()
        );
    }
    assert_eq!(
        fdinfo_mount_id(&Captured {
            bytes: b"pos:\t0\nmnt_id:\t3996\n".to_vec(),
            truncated: false
        })
        .unwrap(),
        Some(3996)
    );
    assert_eq!(
        fdinfo_mount_id(&Captured {
            bytes: b"pos: 0\n".to_vec(),
            truncated: false
        })
        .unwrap(),
        None
    );
    assert!(
        fdinfo_mount_id(&Captured {
            bytes: b"mnt_id: 3996\n".to_vec(),
            truncated: true
        })
        .is_err()
    );
}

#[test]
fn diagnostics_are_bounded_lossless_and_cannot_inject_log_lines() {
    let bytes = b"line\\040with space\n\"\\\xff\x1b[31m\r[codex-mount] forged";
    let capture = read_bounded(bytes.as_slice(), bytes.len()).unwrap();
    assert!(!capture.truncated);
    let mut output = Vec::new();
    write_capture(&mut output, "aliases", "mountinfo", &capture).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert_eq!(text.lines().count(), 3);
    assert!(text.contains("stage=mountinfo-begin"));
    assert!(text.contains("stage=mountinfo-end"));
    assert!(text.contains("truncated=false"));
    assert!(text.contains("\\xff\\x1b[31m\\r[codex-mount] forged"));
    assert!(!text.contains('\x1b'));
    let oversized = vec![b'x'; MOUNTINFO_LIMIT + 1];
    let capture = read_bounded(oversized.as_slice(), MOUNTINFO_LIMIT).unwrap();
    assert!(capture.truncated);
    assert_eq!(capture.bytes.len(), MOUNTINFO_LIMIT);
    let mut output = Vec::new();
    write_capture(&mut output, "control-sockets", "mountinfo", &capture).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.ends_with("truncated=true\n"));
    assert!(
        text.lines()
            .all(|line| line.contains("purpose=control-sockets"))
    );
}

#[test]
fn changing_or_unreadable_namespace_is_rejected() {
    assert!(check_namespace(Ok("mnt:[1]".into()), Ok("mnt:[1]".into())).is_ok());
    assert!(
        check_namespace(Ok("mnt:[1]".into()), Ok("mnt:[2]".into()))
            .unwrap_err()
            .to_string()
            .contains("reason=namespace-changed")
    );
    assert!(
        check_namespace(
            Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            Ok("mnt:[1]".into())
        )
        .unwrap_err()
        .to_string()
        .contains("reason=namespace-read")
    );
}

// Most cases have no independently masked subtree.
fn check_mounts(
    directory: &Path,
    device: &str,
    mount_id: Option<&str>,
    mountinfo: &[u8],
) -> io::Result<BTreeSet<PathBuf>> {
    super::check_mounts(
        directory,
        device,
        SocketFilesystem::Other,
        mount_id,
        mountinfo,
        /*masked_root*/ None,
    )
}

fn check_btrfs_mounts(mount_id: &str, mountinfo: &[u8]) -> io::Result<BTreeSet<PathBuf>> {
    super::check_mounts(
        Path::new("/tmp/codex-daemon-1000"),
        "0:2",
        SocketFilesystem::Btrfs,
        Some(mount_id),
        mountinfo,
        /*masked_root*/ None,
    )
}

#[test]
fn btrfs_bind_mount_masks_aliases_for_both_device_numbers() {
    let mounts = b"1 0 0:1 / / rw - btrfs disk rw\n\
                   2 1 0:1 /var/lib/system-tmp /tmp rw shared:1 - btrfs disk rw\n\
                   3 1 0:1 /var/lib/system-tmp /mount-device-alias rw - btrfs disk rw\n\
                   4 1 0:2 /var/lib/system-tmp /stat-device-alias rw - btrfs disk rw\n\
                   5 1 0:3 mnt:[1234] /run/example.mnt rw - nsfs nsfs rw\n";
    assert_eq!(
        check_btrfs_mounts("2", mounts).unwrap(),
        BTreeSet::from([
            PathBuf::from("/tmp/codex-daemon-1000"),
            PathBuf::from("/var/lib/system-tmp/codex-daemon-1000"),
            PathBuf::from("/mount-device-alias/codex-daemon-1000"),
            PathBuf::from("/stat-device-alias/codex-daemon-1000"),
        ]),
    );
}

#[test_case(SocketFilesystem::Other, "btrfs", Some("1"); "unverified descriptor filesystem")]
#[test_case(SocketFilesystem::Btrfs, "ext4", Some("1"); "inconsistent mount filesystem")]
#[test_case(SocketFilesystem::Btrfs, "btrfs", None; "unavailable mount id")]
#[test_case(SocketFilesystem::Btrfs, "btrfs", Some("missing"); "missing mount id")]
fn device_mismatch_requires_verified_btrfs_mount(
    filesystem: SocketFilesystem,
    mount_filesystem: &str,
    mount_id: Option<&str>,
) {
    let mounts = format!("1 0 0:1 / / rw - {mount_filesystem} disk rw\n");
    assert_eq!(
        super::check_mounts(
            Path::new("/tmp/codex-daemon-1000"),
            "0:2",
            filesystem,
            mount_id,
            mounts.as_bytes(),
            /*masked_root*/ None,
        )
        .map_err(|error| error.kind()),
        Err(io::ErrorKind::Other),
    );
}

#[test_case("0:1", "/@/tmp/codex-daemon-1000", "/alias"; "mount device directory alias")]
#[test_case("0:2", "/@/tmp/codex-daemon-1000/rpc.sock", "/alias.sock"; "stat device socket alias")]
fn btrfs_device_mismatch_still_rejects_unsafe_mounts(device: &str, root: &str, destination: &str) {
    let mounts = format!(
        "1 0 0:1 /@ / rw - btrfs disk rw\n\
         2 1 {device} {root} {destination} rw - btrfs disk rw\n"
    );
    assert_eq!(
        check_btrfs_mounts("1", mounts.as_bytes()).map_err(|error| error.kind()),
        Err(io::ErrorKind::PermissionDenied),
    );
}

#[test]
fn btrfs_device_mismatch_rejects_duplicate_mount_id() {
    let mounts = b"1 0 0:1 /@ / rw - btrfs disk rw\n\
                   1 0 0:1 /@ / rw - btrfs disk rw\n";
    assert_eq!(
        check_btrfs_mounts("1", mounts).map_err(|error| error.kind()),
        Err(io::ErrorKind::Other),
    );
}

#[test_case("/tmp", "/host-tmp", true; "ancestor alias")]
#[test_case("/tmp/codex-daemon-1000", "/alias", false; "directory alias")]
#[test_case("/tmp/codex-daemon-1000/rpc.sock", "/alias.sock", false; "socket alias")]
#[test_case("/", "/host", true; "root alias")]
#[test_case("/workspace", "/project", true; "unrelated project bind")]
#[test_case("/tmp", "/tmp", true; "same location")]
#[test_case("/tmp", "/host\\040tmp", true; "escaped alias")]
#[test_case("/other", "/tmp/codex-daemon-1000/nested", false; "nested mount")]
fn accepts_only_mounts_that_can_keep_the_directory_masked(
    root: &str,
    destination: &str,
    allowed: bool,
) {
    let mounts =
        format!("1 0 0:1 / / rw - ext4 disk rw\n2 1 0:1 {root} {destination} rw - ext4 disk rw\n");
    let visible_mount = if destination == "/tmp" { "2" } else { "1" };
    for mount_id in [Some(visible_mount), None] {
        assert_eq!(
            check_mounts(
                Path::new("/tmp/codex-daemon-1000"),
                "0:1",
                mount_id,
                mounts.as_bytes()
            )
            .is_ok(),
            allowed,
            "mount_id: {mount_id:?}"
        );
    }
}

#[test_case("0:2", "mnt:[4026532835]", "/run/snapd/ns/example.mnt", Ok(()); "unrelated mount namespace")]
#[test_case("0:2", "net:[4026531840]", "/run/netns/example", Ok(()); "unrelated network namespace")]
#[test_case("0:2", "mnt:[4026532835]", "/tmp/codex-daemon-1000/ns", Err(io::ErrorKind::PermissionDenied); "nested namespace mount")]
#[test_case("0:1", "mnt:[4026532835]", "/run/snapd/ns/example.mnt", Err(io::ErrorKind::Other); "non-path root on socket filesystem")]
#[test_case("0:2", "mnt:[4026532835]", "relative/ns", Err(io::ErrorKind::Other); "relative destination")]
#[test_case("0:2", "mnt:[4026532835]", "/run/snapd/ns/\\invalid", Err(io::ErrorKind::Other); "invalid destination escape")]
fn validates_namespace_mounts_by_device_and_destination(
    device: &str,
    root: &str,
    destination: &str,
    expected: Result<(), io::ErrorKind>,
) {
    let mounts = format!(
        "1 0 0:1 / / rw - ext4 disk rw\n2 1 {device} {root} {destination} rw - nsfs nsfs rw\n"
    );
    for mount_id in [Some("1"), None] {
        assert_eq!(
            check_mounts(
                Path::new("/tmp/codex-daemon-1000"),
                "0:1",
                mount_id,
                mounts.as_bytes()
            )
            .map(|_| ())
            .map_err(|error| error.kind()),
            expected,
            "mount_id: {mount_id:?}"
        );
    }
}

#[test]
fn unrelated_namespace_mount_does_not_hide_a_socket_alias() {
    let mounts = b"1 0 0:1 / / rw - ext4 disk rw\n\
                   2 1 0:2 net:[4026531840] /run/netns/example rw - nsfs nsfs rw\n\
                   3 1 0:1 /tmp/codex-daemon-1000 /alias rw - ext4 disk rw\n";
    for mount_id in [Some("1"), None] {
        assert_eq!(
            check_mounts(Path::new("/tmp/codex-daemon-1000"), "0:1", mount_id, mounts)
                .map_err(|error| error.kind()),
            Err(io::ErrorKind::PermissionDenied),
            "mount_id: {mount_id:?}"
        );
    }
}

#[test]
fn masks_alias_when_tmp_is_itself_a_bind_mount() {
    let mounts = b"1 0 0:1 / / rw - ext4 disk rw\n2 1 0:1 /backing/tmp /tmp rw - ext4 disk rw\n";
    let expected = BTreeSet::from([
        PathBuf::from("/tmp/codex-daemon-1000"),
        PathBuf::from("/backing/tmp/codex-daemon-1000"),
    ]);
    assert_eq!(
        check_mounts(
            Path::new("/tmp/codex-daemon-1000"),
            "0:1",
            Some("2"),
            mounts
        )
        .unwrap(),
        expected
    );
    // A hidden deeper mount must not override the actual /tmp backing location.
    let hidden = [
        mounts.as_slice(),
        b"3 1 0:1 /tmp/codex-daemon-1000 /tmp/codex-daemon-1000 rw - ext4 disk rw\n",
    ]
    .concat();
    assert_eq!(
        check_mounts(
            Path::new("/tmp/codex-daemon-1000"),
            "0:1",
            Some("2"),
            &hidden
        )
        .unwrap(),
        expected
    );
}

#[test]
fn accepts_private_tmp_filesystem_and_resolves_stacked_mounts() {
    let mounts = "1 0 0:1 / / rw - ext4 disk rw\n2 1 0:2 / /tmp rw - tmpfs tmpfs rw\n";
    let directory = Path::new("/tmp/codex-daemon-1000");
    assert!(check_mounts(directory, "0:2", Some("2"), mounts.as_bytes()).is_ok());
    assert!(check_mounts(directory, "0:2", /*mount_id*/ None, mounts.as_bytes()).is_ok());
    // Missing or inconsistent precise IDs must not fall back to the otherwise
    // acceptable conservative interpretation.
    assert!(check_mounts(directory, "0:2", Some("missing"), mounts.as_bytes()).is_err());
    assert!(check_mounts(directory, "0:2", Some("1"), mounts.as_bytes()).is_err());
    assert!(check_mounts(directory, "0:3", /*mount_id*/ None, mounts.as_bytes()).is_err());
    let stacked = format!("{mounts}3 2 0:2 /other /tmp rw - tmpfs tmpfs rw\n");
    assert!(check_mounts(directory, "0:2", Some("3"), stacked.as_bytes()).is_ok());
    assert!(check_mounts(directory, "0:2", /*mount_id*/ None, stacked.as_bytes()).is_err());
}

#[test]
fn rejects_open_mount_that_has_been_covered() {
    let directory = Path::new("/tmp/codex-daemon-1000");
    let mounts = "1 0 0:1 / / rw - ext4 disk rw\n\
                  2 1 0:1 /tmp/private-old/tmp /tmp rw - ext4 disk rw\n\
                  3 2 0:1 /tmp/private-new/tmp /tmp rw - ext4 disk rw\n";
    assert!(check_mounts(directory, "0:1", Some("2"), mounts.as_bytes()).is_err());
    assert!(check_mounts(directory, "0:1", Some("3"), mounts.as_bytes()).is_ok());
    let exposed = format!(
        "{mounts}4 1 0:1 /tmp/private-new/tmp/codex-daemon-1000 /outside rw - ext4 disk rw\n"
    );
    for mount_id in [Some("2"), Some("3"), None] {
        assert!(check_mounts(directory, "0:1", mount_id, exposed.as_bytes()).is_err());
    }
}

#[test]
fn rejects_mount_hidden_by_an_ancestor_overmount() {
    let directory = Path::new("/tmp/private/codex-daemon-1000");
    let mounts = "1 0 0:1 / / rw - ext4 disk rw\n\
                  2 1 0:2 / /tmp rw - tmpfs tmpfs rw\n\
                  3 2 0:3 / /tmp/private rw - tmpfs tmpfs rw\n";
    assert!(check_mounts(directory, "0:3", Some("3"), mounts.as_bytes()).is_ok());
    let covered = format!("{mounts}4 2 0:2 /other /tmp rw - tmpfs tmpfs rw\n");
    assert!(check_mounts(directory, "0:3", Some("3"), covered.as_bytes()).is_err());
    assert!(check_mounts(directory, "0:2", Some("4"), covered.as_bytes()).is_ok());
}

#[test_case("/tmp/systemd-private-service/tmp", "/"; "tmp on root filesystem")]
#[test_case("/systemd-private-service/tmp", "/tmp"; "tmp on separate filesystem")]
fn accepts_private_tmp_bind_and_maskable_aliases(root: &str, parent: &str) {
    let directory = Path::new("/tmp/codex-daemon-1000");
    let mounts =
        format!("1 0 0:1 / {parent} rw - ext4 disk rw\n2 1 0:1 {root} /tmp rw - ext4 disk rw\n");
    assert!(check_mounts(directory, "0:1", Some("2"), mounts.as_bytes()).is_ok());
    // PrivateTmp remains ambiguous when neither fdinfo nor statx supplies an ID.
    assert!(check_mounts(directory, "0:1", /*mount_id*/ None, mounts.as_bytes()).is_err());

    // A real alias beneath /tmp needs a mask; a direct socket alias remains unsupported.
    for (alias_root, destination, can_mask) in [
        (root.to_owned(), "/tmp/exposed", true),
        (
            format!("{root}/codex-daemon-1000/rpc.sock"),
            "/tmp/alias.sock",
            false,
        ),
        ("/".to_owned(), "/host", true),
    ] {
        let exposed = format!("{mounts}3 2 0:1 {alias_root} {destination} rw - ext4 disk rw\n");
        assert_eq!(
            check_mounts(directory, "0:1", Some("2"), exposed.as_bytes()).is_ok(),
            can_mask
        );
    }
}

#[test]
fn masked_wslg_alias_does_not_skip_other_socket_masks() {
    let mounts = "1 0 0:1 / / rw - ext4 disk rw\n2 1 0:1 / /mnt/wslg/distro rw - ext4 disk rw\n";
    let directory = Path::new("/tmp/codex-daemon-1000");
    let mask = Some(Path::new("/mnt/wslg/distro"));
    let exposed = format!("{mounts}3 1 0:1 /tmp /host-tmp rw - ext4 disk rw\n");
    for mount_id in [Some("1"), None] {
        assert_eq!(
            check_mounts(directory, "0:1", mount_id, mounts.as_bytes()).unwrap(),
            BTreeSet::from([
                directory.to_path_buf(),
                PathBuf::from("/mnt/wslg/distro/tmp/codex-daemon-1000"),
            ])
        );
        assert_eq!(
            super::check_mounts(
                directory,
                "0:1",
                SocketFilesystem::Other,
                mount_id,
                mounts.as_bytes(),
                mask
            )
            .unwrap(),
            BTreeSet::from([directory.to_path_buf()])
        );
        assert_eq!(
            super::check_mounts(
                directory,
                "0:1",
                SocketFilesystem::Other,
                mount_id,
                exposed.as_bytes(),
                mask
            )
            .unwrap(),
            BTreeSet::from([
                directory.to_path_buf(),
                PathBuf::from("/host-tmp/codex-daemon-1000"),
            ])
        );
    }
}

#[test]
fn ohos_account_root_masks_storage_mount_aliases() {
    let mounts = b"1 0 8:1 / / rw - ext4 system rw\n\
        2 1 8:2 / /storage/Users/currentUser rw - ext4 users rw\n\
        3 1 8:2 / /user-alias rw - ext4 users rw\n";
    let directory = Path::new("/storage/Users/currentUser/.codex-uds");
    assert_eq!(
        check_mounts(directory, "8:2", Some("2"), mounts).unwrap(),
        BTreeSet::from([
            directory.to_path_buf(),
            PathBuf::from("/user-alias/.codex-uds")
        ])
    );
}

#[test]
fn ohos_account_socket_alias_cannot_bypass_the_mask() {
    let mounts = b"1 0 8:1 / / rw - ext4 system rw\n\
        2 1 8:2 / /storage/Users/currentUser rw - ext4 users rw\n\
        3 1 8:2 /.codex-uds/rpc /exposed.sock rw - ext4 users rw\n";
    assert_eq!(
        check_mounts(
            Path::new("/storage/Users/currentUser/.codex-uds"),
            "8:2",
            Some("2"),
            mounts
        )
        .expect_err("direct socket bind alias must fail closed")
        .kind(),
        io::ErrorKind::PermissionDenied
    );
}
