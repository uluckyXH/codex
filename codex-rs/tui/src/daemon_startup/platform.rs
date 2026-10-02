//! Platform policy for local shared daemons. Embedded sessions need no daemon package.

pub(crate) const fn local_daemon_supported() -> bool {
    !cfg!(target_env = "ohos")
}

pub(crate) fn validate_request(
    supported: bool,
    agents_overview: bool,
    auto_start_enabled: bool,
    no_daemon: bool,
    explicit_remote: bool,
) -> std::io::Result<()> {
    if supported || explicit_remote {
        return Ok(());
    }
    let reason = if agents_overview {
        "The local shared daemon required by `codex agents` is unavailable on HarmonyOS. Run `codex` for a local embedded session."
    } else if auto_start_enabled && !no_daemon {
        "features.daemon_auto_start=true is unsupported on HarmonyOS. Set it to false or use --no-daemon for a local embedded session."
    } else {
        return Ok(());
    };
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, reason))
}

#[cfg(test)]
mod tests {
    use super::validate_request;
    use std::io::ErrorKind;

    #[test]
    fn harmony_default_and_no_daemon_use_embedded_sessions() {
        assert!(validate_request(false, false, false, false, false).is_ok());
        assert!(validate_request(false, false, true, true, false).is_ok());
    }

    #[test]
    fn harmony_explicit_auto_start_reports_the_setting_and_recovery() {
        let error = validate_request(false, false, true, false, false).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Unsupported);
        let message = error.to_string();
        assert!(message.contains("features.daemon_auto_start=true"));
        assert!(message.contains("--no-daemon"));
    }

    #[test]
    fn harmony_local_agents_overview_requires_a_supported_daemon() {
        for no_daemon in [false, true] {
            let error = validate_request(false, true, false, no_daemon, false).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Unsupported);
            assert!(error.to_string().contains("codex agents"));
        }
    }

    #[test]
    fn existing_remote_and_other_platform_launches_are_preserved() {
        for (supported, remote) in [(true, false), (true, true), (false, true)] {
            for overview in [false, true] {
                for auto_start in [false, true] {
                    assert!(
                        validate_request(supported, overview, auto_start, false, remote).is_ok()
                    );
                }
            }
        }
    }
}
