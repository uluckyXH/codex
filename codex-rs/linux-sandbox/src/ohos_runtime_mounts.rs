//! Restore only the readable alias directory after /dev and writable binds.
//! The sibling privileged socket directory must never be restored with it.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;
use std::path::PathBuf;

pub(crate) fn append_alias_readonly_mounts(
    args: &mut Vec<String>,
    aliases: &BTreeSet<PathBuf>,
    can_read: impl Fn(&Path) -> bool,
    denied: &[PathBuf],
    writable: &[PathBuf],
) -> io::Result<()> {
    let mut mounts = Vec::new();
    for alias in aliases {
        // A read-only child mount does not pin every ancestor's name. Allowing
        // a writable ancestor could let a restricted command rename the parent
        // of both a/ and s/, then create a new unmasked runtime tree at the old
        // path for a later listener. Reject that policy rather than bind the
        // entire privileged parent back into the sandbox.
        if writable
            .iter()
            .any(|root| root.starts_with(alias) || alias.starts_with(root))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "OHOS runtime: writable roots must not overlap aliases or their ancestors",
            ));
        }
        if !can_read(alias) {
            continue;
        }
        // Conservatively reject conflicting carveouts rather than undoing an
        // earlier deny mount when the alias tree is rebound from the host.
        if denied
            .iter()
            .any(|path| path.starts_with(alias) || alias.starts_with(path))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "OHOS aliases: read-only restoration conflicts with an unreadable path",
            ));
        }
        let path = alias.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "OHOS aliases: non-UTF-8 mount path",
            )
        })?;
        mounts.extend(["--ro-bind".to_string(), path.to_string(), path.to_string()]);
    }
    args.extend(mounts);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_recreation_restores_only_aliases_readonly() {
        let alias = PathBuf::from("/dev/shm/c12345678/a");
        let mut args = vec![
            "--ro-bind".into(),
            "/".into(),
            "/".into(),
            "--dev".into(),
            "/dev".into(),
            "--tmpfs".into(),
            "/dev/shm/c12345678/s".into(),
        ];
        let prefix = args.clone();
        append_alias_readonly_mounts(
            &mut args,
            &BTreeSet::from([alias.clone()]),
            |_| true,
            &[],
            &[PathBuf::from("/project")],
        )
        .unwrap();
        assert_eq!(&args[..prefix.len()], &prefix);
        assert_eq!(
            &args[prefix.len()..],
            &[
                "--ro-bind",
                alias.to_str().unwrap(),
                alias.to_str().unwrap()
            ]
        );
    }

    #[test]
    fn conflicting_rules_reject_without_partial_mounts() {
        let alias = PathBuf::from("/runtime/a");
        for (denied, writable) in [
            (vec![PathBuf::from("/runtime")], vec![]),
            (vec![alias.join("session")], vec![]),
            (vec![], vec![alias.clone()]),
            (vec![], vec![alias.join("session")]),
            (vec![], vec![PathBuf::from("/runtime")]),
            (vec![], vec![PathBuf::from("/")]),
        ] {
            let mut args = Vec::new();
            assert!(
                append_alias_readonly_mounts(
                    &mut args,
                    &BTreeSet::from([alias.clone()]),
                    |_| true,
                    &denied,
                    &writable
                )
                .is_err()
            );
            assert!(args.is_empty());
        }
    }

    #[test]
    fn unreadable_alias_is_not_reopened_and_writable_exposure_is_rejected() {
        let aliases = BTreeSet::from([PathBuf::from("/runtime/a")]);
        let mut args = Vec::new();
        append_alias_readonly_mounts(&mut args, &aliases, |_| false, &[], &[]).unwrap();
        assert!(args.is_empty());
        assert!(
            append_alias_readonly_mounts(
                &mut args,
                &aliases,
                |_| false,
                &[],
                &[PathBuf::from("/")]
            )
            .is_err()
        );
        assert!(args.is_empty());
    }

    #[test]
    fn every_visible_alias_receives_its_own_protection() {
        let aliases = BTreeSet::from([
            PathBuf::from("/runtime/a"),
            PathBuf::from("/view/runtime/a"),
        ]);
        let mut args = Vec::new();
        append_alias_readonly_mounts(&mut args, &aliases, |_| true, &[], &[]).unwrap();
        assert_eq!(
            args,
            [
                "--ro-bind",
                "/runtime/a",
                "/runtime/a",
                "--ro-bind",
                "/view/runtime/a",
                "/view/runtime/a"
            ]
        );
    }
    #[test]
    fn writable_mount_alias_ancestor_is_rejected_before_any_restoration() {
        let aliases = BTreeSet::from([
            PathBuf::from("/data/storage/el2/base/files/c12345678/a"),
            PathBuf::from("/mapped/files/c12345678/a"),
        ]);
        let mut args = vec!["existing-mask".to_string()];
        let result = append_alias_readonly_mounts(
            &mut args,
            &aliases,
            |_| true,
            &[],
            &[PathBuf::from("/mapped/files")],
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(args, ["existing-mask"]);
    }
}
