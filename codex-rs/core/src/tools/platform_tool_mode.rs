//! Resolve the tool mode without allowing model metadata to require a missing host.

use codex_protocol::openai_models::ToolMode;

pub(super) fn resolve_requested_tool_mode(
    model_mode: Option<ToolMode>,
    code_only: bool,
    code: bool,
    supported: bool,
) -> ToolMode {
    if !supported {
        // Model metadata must not hide direct tools behind an unavailable V8 host.
        return ToolMode::Direct;
    }
    model_mode.unwrap_or_else(|| {
        if code_only {
            ToolMode::CodeModeOnly
        } else if code {
            ToolMode::CodeMode
        } else {
            ToolMode::Direct
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_host_cannot_hide_direct_tools_via_model_metadata() {
        for mode in [
            None,
            Some(ToolMode::Direct),
            Some(ToolMode::CodeMode),
            Some(ToolMode::CodeModeOnly),
        ] {
            assert_eq!(
                resolve_requested_tool_mode(mode, false, false, false),
                ToolMode::Direct
            );
            assert_eq!(
                resolve_requested_tool_mode(mode, true, true, false),
                ToolMode::Direct
            );
        }
    }

    #[test]
    fn supported_targets_keep_model_and_config_precedence() {
        assert_eq!(
            resolve_requested_tool_mode(Some(ToolMode::CodeModeOnly), false, false, true),
            ToolMode::CodeModeOnly
        );
        assert_eq!(
            resolve_requested_tool_mode(Some(ToolMode::Direct), true, true, true),
            ToolMode::Direct
        );
        assert_eq!(
            resolve_requested_tool_mode(None, false, true, true),
            ToolMode::CodeMode
        );
    }
}
