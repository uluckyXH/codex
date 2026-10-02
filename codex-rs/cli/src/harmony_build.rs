//! Compile-time provenance, independent of user configuration or credentials.

pub(crate) fn build_id() -> &'static str {
    checked_build_id(option_env!("CODEX_HARMONY_BUILD_ID"))
}

fn checked_build_id(value: Option<&str>) -> &str {
    match value {
        Some(value)
            if !value.is_empty()
                && value.len() <= 128
                && value.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+')
                }) =>
        {
            value
        }
        Some(_) => "invalid",
        None => "unknown",
    }
}

pub(crate) fn cli_version() -> &'static str {
    if cfg!(target_env = "ohos") {
        static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        VERSION.get_or_init(|| harmony_version(env!("CARGO_PKG_VERSION"), build_id(), target()))
    } else {
        env!("CARGO_PKG_VERSION")
    }
}

fn harmony_version(version: &str, build_id: &str, target: &str) -> String {
    format!("{version} (HarmonyOS; build {build_id}; target {target})")
}

pub(crate) fn target() -> &'static str {
    option_env!("CODEX_CLI_BUILD_TARGET").unwrap_or("unknown")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_identity_distinguishes_missing_and_invalid_values() {
        assert_eq!(checked_build_id(None), "unknown");
        assert_eq!(checked_build_id(Some("")), "invalid");
        assert_eq!(checked_build_id(Some("pc7-a1b2c3")), "pc7-a1b2c3");
        assert_eq!(checked_build_id(Some("\u{1b}[31muntrusted\n")), "invalid");
        assert_eq!(checked_build_id(Some(&"a".repeat(129))), "invalid");
    }

    #[test]
    fn harmony_version_includes_candidate_identity_and_actual_target() {
        assert_eq!(
            harmony_version("0.0.0", "pc7-a1b2c3", "aarch64-unknown-linux-ohos"),
            "0.0.0 (HarmonyOS; build pc7-a1b2c3; target aarch64-unknown-linux-ohos)"
        );
    }

    #[test]
    fn version_keeps_upstream_version_and_target_provenance() {
        assert!(cli_version().starts_with(env!("CARGO_PKG_VERSION")));
        if cfg!(target_env = "ohos") {
            assert!(cli_version().contains("HarmonyOS; build "));
            assert!(cli_version().contains(target()));
        } else {
            assert_eq!(cli_version(), env!("CARGO_PKG_VERSION"));
        }
    }
}
