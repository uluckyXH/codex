#![allow(dead_code)]
mod codex_tui {
    pub fn get_update_action() -> Option<()> { panic!("update discovery must not run"); }
}
fn run_update_action(_: (), _: Option<&std::path::Path>) -> anyhow::Result<()> {
    panic!("update execution must not run");
}
fn run_update_command() -> anyhow::Result<()> {
    anyhow::ensure!(
        !cfg!(target_env = "ohos"),
        "Updates are unavailable for this HarmonyOS build. Install a compatible HarmonyOS release manually."
    );
    #[cfg(debug_assertions)]
    {
        anyhow::bail!(
            "`codex update` is not available in debug builds. Install a release build of Codex to use this command."
        );
    }

    #[cfg(not(debug_assertions))]
    {
        let Some(action) = codex_tui::get_update_action() else {
            anyhow::bail!(
                "Could not detect the Codex installation method. Please update manually: https://developers.openai.com/codex/cli/"
            );
        };
        run_update_action(action, /*cli_executable*/ None)
    }
}

#[test]
fn harmony_refuses_updates_before_discovery_or_execution() {
    assert!(cfg!(target_env = "ohos"));
    let error = run_update_command().unwrap_err().to_string();
    assert!(error.contains("Updates are unavailable for this HarmonyOS build"));
    assert!(!error.contains("https://"));
}
