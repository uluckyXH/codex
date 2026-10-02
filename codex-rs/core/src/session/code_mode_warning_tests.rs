use super::unsupported_code_mode_warning;
use codex_features::Feature;
use codex_features::Features;
use codex_models_manager::model_info::model_info_from_slug;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ToolMode;
use pretty_assertions::assert_eq;

const MODEL_SLUG: &str = "test-model";

fn known_model_info() -> ModelInfo {
    ModelInfo {
        used_fallback_model_metadata: false,
        ..model_info_from_slug(MODEL_SLUG)
    }
}

#[test]
#[cfg(not(target_env = "ohos"))]
fn warns_when_code_mode_is_enabled_without_model_selector() {
    let mut features = Features::with_defaults();
    features.enable(Feature::CodeMode);

    assert_eq!(
        unsupported_code_mode_warning(&known_model_info(), &features),
        Some(format!(
            "Code Mode is enabled in configuration, but model `{MODEL_SLUG}` does not advertise Code Mode support. This may degrade model performance. Disable `features.code_mode` and `features.code_mode_only`, or select a model whose metadata enables Code Mode."
        ))
    );
}

#[test]
#[cfg(not(target_env = "ohos"))]
fn warns_when_code_mode_only_is_enabled_without_model_selector() {
    let mut features = Features::with_defaults();
    features.enable(Feature::CodeModeOnly);

    assert!(unsupported_code_mode_warning(&known_model_info(), &features).is_some());
}

#[test]
fn does_not_warn_when_code_mode_is_disabled() {
    assert_eq!(
        unsupported_code_mode_warning(&known_model_info(), &Features::with_defaults()),
        None
    );
}

#[test]
#[cfg(not(target_env = "ohos"))]
fn does_not_warn_when_model_has_tool_mode_selector() {
    let mut features = Features::with_defaults();
    features.enable(Feature::CodeModeOnly);

    for tool_mode in [ToolMode::Direct, ToolMode::CodeMode, ToolMode::CodeModeOnly] {
        let model_info = ModelInfo {
            tool_mode: Some(tool_mode),
            ..known_model_info()
        };
        assert_eq!(unsupported_code_mode_warning(&model_info, &features), None);
    }
}

#[cfg(target_env = "ohos")]
#[test]
fn harmony_explains_direct_tools_for_code_mode_model_metadata() {
    for tool_mode in [ToolMode::CodeMode, ToolMode::CodeModeOnly] {
        let model_info = ModelInfo {
            tool_mode: Some(tool_mode),
            ..known_model_info()
        };
        let warning =
            unsupported_code_mode_warning(&model_info, &Features::with_defaults()).unwrap();
        assert!(warning.contains("Direct tools will be used"));
        assert!(warning.contains("HarmonyOS"));
    }
}

#[test]
fn fallback_metadata_only_uses_existing_warning() {
    let mut features = Features::with_defaults();
    features.enable(Feature::CodeMode);

    assert_eq!(
        unsupported_code_mode_warning(&model_info_from_slug(MODEL_SLUG), &features),
        None
    );
}
