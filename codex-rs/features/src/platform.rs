//! Capabilities of the native CLI build, independent of configuration and model metadata.
//! A permission or a feature toggle cannot supply a missing platform implementation.

use crate::Feature;

pub const CODE_MODE_UNAVAILABLE: &str =
    "the V8 code-mode host is not available in this native HarmonyOS build; use direct tools";
pub const ZSH_FORK_UNAVAILABLE: &str =
    "the customized zsh exec bridge is not available on HarmonyOS; use the native shell tools";
pub const VOICE_UNAVAILABLE: &str =
    "native voice capture and playback are not implemented for the HarmonyOS CLI";
pub const DESKTOP_UNAVAILABLE: &str =
    "desktop and browser automation require a HarmonyOS integration that this CLI does not provide";
pub const BROWSER_OPEN_UNAVAILABLE: &str = "this CLI has no native browser launcher; the examined public API 24 Ability C interfaces require an application context";

pub fn unavailable_reason(feature: Feature) -> Option<&'static str> {
    unavailable_reason_for(feature, cfg!(target_env = "ohos"))
}

fn unavailable_reason_for(feature: Feature, ohos: bool) -> Option<&'static str> {
    if !ohos {
        return None;
    }
    match feature {
        Feature::CodeMode
        | Feature::CodeModeHost
        | Feature::CodeModePrewarm
        | Feature::CodeModeInterrupt
        | Feature::CodeModeOnly => Some(CODE_MODE_UNAVAILABLE),
        Feature::ShellZshFork | Feature::UnifiedExecZshFork => Some(ZSH_FORK_UNAVAILABLE),
        Feature::RealtimeConversation | Feature::InAppVoice | Feature::InAppDictation => {
            Some(VOICE_UNAVAILABLE)
        }
        Feature::ComputerUse
        | Feature::BrowserUse
        | Feature::BrowserUseFullCdpAccess
        | Feature::BrowserUseExternal
        | Feature::InAppBrowser
        | Feature::BrowserAnnotationApi
        | Feature::InAppLocalAutomation => Some(DESKTOP_UNAVAILABLE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FEATURES;
    use crate::Features;

    #[test]
    fn harmony_keeps_direct_local_tools_and_rejects_missing_hosts() {
        for feature in [
            Feature::ShellTool,
            Feature::UnifiedExec,
            Feature::UnifiedExecTty,
            Feature::ViewImage,
            Feature::ApplyPatchPreserveLineEndings,
        ] {
            assert!(unavailable_reason_for(feature, true).is_none());
        }
        for feature in [
            Feature::CodeMode,
            Feature::CodeModeHost,
            Feature::CodeModePrewarm,
            Feature::CodeModeOnly,
            Feature::ShellZshFork,
            Feature::RealtimeConversation,
            Feature::ComputerUse,
            Feature::BrowserUse,
            Feature::InAppVoice,
        ] {
            assert!(unavailable_reason_for(feature, true).is_some());
        }
    }

    #[test]
    fn other_targets_keep_their_capabilities() {
        for spec in FEATURES {
            assert!(unavailable_reason_for(spec.id, false).is_none());
        }
    }

    #[test]
    fn boolean_and_structured_feature_requests_are_retained_for_validation() {
        for config in [
            "code_mode = true",
            "[code_mode]\nenabled = true",
            "[code_mode_host]\nenabled = true\ndisable_in_process_fallback = true",
            "code_mode_prewarm = true",
            "shell_zsh_fork = true",
            "realtime_conversation = true",
            "computer_use = true",
        ] {
            let table: crate::FeaturesToml = toml::from_str(config).unwrap();
            let mut features = Features::default();
            features.apply_toml(&table);
            assert!(
                features
                    .enabled
                    .iter()
                    .any(|feature| unavailable_reason_for(*feature, true).is_some())
            );
        }
        let table: crate::FeaturesToml =
            toml::from_str("code_mode_host = false\nrealtime_conversation = false").unwrap();
        let mut features = Features::default();
        features.apply_toml(&table);
        assert!(features.enabled.is_empty());
    }

    #[test]
    fn defaults_and_internal_toggles_respect_the_compiled_target() {
        let defaults = Features::with_defaults();
        assert!(defaults.unavailable_enabled_feature().is_none());
        for spec in FEATURES {
            if unavailable_reason(spec.id).is_some() {
                assert!(!spec.default_enabled);
                let mut requested = defaults.clone();
                requested.enable(spec.id);
                assert!(!requested.enabled(spec.id));
                assert!(requested.unavailable_enabled_feature().is_some());
                assert!(!requested.enabled_features().contains(&spec.id));
            }
        }
    }
}
