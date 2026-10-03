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
        // An explicitly writable session could undo alias integrity. Broader
        // writable roots are allowed only with this final read-only overlay.
        if writable.iter().any(|root| root.starts_with(alias)) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "OHOS aliases: a writable root overlaps a reserved alias directory",
            ));
        }
        let readable = can_read(alias);
        if !readable {
            if writable.iter().any(|root| alias.starts_with(root)) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "OHOS aliases: cannot protect aliases without reopening a denied path",
                ));
            }
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
    fn dev_recreation_and_writable_ancestor_restore_only_aliases_readonly() {
        let alias = PathBuf::from("/dev/shm/c12345678/a");
        let mut args = vec![
            "--dev".into(),
            "/dev".into(),
            "--bind".into(),
            "/dev/shm".into(),
            "/dev/shm".into(),
            "--tmpfs".into(),
            "/dev/shm/c12345678/s".into(),
        ];
        let prefix = args.clone();
        append_alias_readonly_mounts(
            &mut args,
            &BTreeSet::from([alias.clone()]),
            |_| true,
            &[],
            &[PathBuf::from("/dev/shm")],
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
}
