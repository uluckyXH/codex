//! Capability-only diagnostics never load Config, auth, clipboard data or devices.

use super::CheckStatus;
use super::DoctorCheck;
use super::DoctorReport;
use codex_features::FEATURES;

pub(super) fn capabilities_check() -> DoctorCheck {
    let mut details = Vec::new();
    for spec in FEATURES {
        if let Some(reason) = codex_features::platform::unavailable_reason(spec.id) {
            details.push(format!("features.{}: unavailable; {reason}", spec.key));
        }
    }
    if cfg!(target_env = "ohos") {
        let shell = codex_core::shell::default_user_shell();
        details.push(format!(
            "local shell type: {}; execution not probed",
            shell.name()
        ));
        details.extend([
            "local execution: embedded app-server and direct shell/file tools; execution not probed".to_owned(),
            "text clipboard: public Pasteboard/UDMF API 13 backend implemented; library availability and process permission not probed".to_owned(),
            "clipboard read: requires ohos.permission.READ_PASTEBOARD; use terminal paste when the process lacks permission".to_owned(),
            "native text paste: opt in with tui.right_click_paste = \"on\"; automatic right-click paste remains disabled".to_owned(),
            "clipboard copy fallback: OSC 52 request is unconfirmed until pasted".to_owned(),
            "clipboard image/HTML: native integration not implemented".to_owned(),
            format!("browser opening: unavailable; {}", codex_features::platform::BROWSER_OPEN_UNAVAILABLE),
            "local shared daemon and automatic updates: unavailable".to_owned(),
        ]);
    }
    DoctorCheck::new(
        "runtime.capabilities",
        "runtime",
        CheckStatus::Ok,
        if cfg!(target_env = "ohos") {
            "native HarmonyOS capability policy; runtime support is not verified"
        } else {
            "no HarmonyOS capability restrictions apply to this target"
        },
    )
    .details(details)
}

pub(super) fn report() -> DoctorReport {
    let checks = vec![
        super::runtime_check(),
        super::search_check(),
        capabilities_check(),
    ];
    DoctorReport {
        schema_version: 1,
        generated_at: chrono::Utc::now().to_rfc3339(),
        overall_status: checks
            .iter()
            .map(|check| check.status)
            .max()
            .unwrap_or(CheckStatus::Ok),
        codex_version: env!("CARGO_PKG_VERSION").to_owned(),
        checks,
    }
}

#[cfg(test)]
#[test]
fn capability_report_has_no_auth_or_network_probe() {
    let report = report();
    assert_eq!(report.checks.len(), 3);
    assert!(
        report
            .checks
            .iter()
            .all(|check| check.id.starts_with("runtime."))
    );
    if cfg!(target_env = "ohos") {
        let detail = report.checks[2].details.join("\n");
        assert!(detail.contains("READ_PASTEBOARD"));
        assert!(detail.contains("features.code_mode_host: unavailable"));
        assert!(detail.contains("not probed"));
    }
}
