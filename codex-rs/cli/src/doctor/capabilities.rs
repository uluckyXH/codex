//! Capability-only diagnostics never load Config, auth, clipboard data or devices.
//! On OHOS, the standard capability-only invocation bypasses dotenv and aliases.

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
    #[cfg(target_env = "ohos")]
    details.extend(codex_sandboxing::bwrap_resource_diagnostics());
    #[cfg(target_env = "ohos")]
    let (status, contract_details) = runtime_contract(codex_uds::ohos_runtime_base_contract());
    #[cfg(target_env = "ohos")]
    details.extend(contract_details);
    #[cfg(not(target_env = "ohos"))]
    let status = CheckStatus::Ok;
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
        status,
        if cfg!(target_env = "ohos") {
            "native HarmonyOS capability policy; runtime support is not verified"
        } else {
            "no HarmonyOS capability restrictions apply to this target"
        },
    )
    .details(details)
}

#[cfg(any(target_env = "ohos", test))]
fn runtime_contract(base: Option<&str>) -> (CheckStatus, Vec<String>) {
    let (status, contract) = match base {
        Some(base) => (
            CheckStatus::Warning,
            format!("runtime directory build contract: CODEX_OHOS_RUNTIME_BASE={base}"),
        ),
        None => (
            CheckStatus::Fail,
            "runtime directory build contract: not set; normal startup is blocked".to_owned(),
        ),
    };
    (status, vec![
        contract,
        "runtime directory selection: build-bound; no HOME/CODEX_HOME/TMPDIR/Context fallback".to_owned(),
        "runtime directory validation: commercial HarmonyOS PC acceptance pending; this report does not approve a safe root or create runtime directories".to_owned(),
        "runtime layout: base/c<effective-uid-hex>/a for executable aliases; base/c<effective-uid-hex>/s for protected control sockets and locks".to_owned(),
        "runtime path evidence: run codex-resources/harmony-runtime-probe independently; native context and mount semantics are not probed by doctor --capabilities".to_owned(),
    ])
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
        codex_version: crate::harmony_build::cli_version().to_owned(),
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

#[cfg(test)]
#[test]
fn runtime_contract_is_diagnostic_and_never_claims_device_acceptance() {
    let (missing, missing_details) = runtime_contract(None);
    assert_eq!(missing, CheckStatus::Fail);
    assert!(
        missing_details
            .join("\n")
            .contains("normal startup is blocked")
    );
    let (bound, details) = runtime_contract(Some("/fixed/fixture"));
    assert_eq!(bound, CheckStatus::Warning);
    let details = details.join("\n");
    assert!(details.contains("CODEX_OHOS_RUNTIME_BASE=/fixed/fixture"));
    assert!(details.contains("acceptance pending"));
    assert!(details.contains("no HOME/CODEX_HOME/TMPDIR/Context fallback"));
}
