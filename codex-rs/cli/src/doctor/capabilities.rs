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
    let (status, contract_details) = runtime_contract(
        codex_uds::ohos_runtime_profile_contract(),
        codex_uds::ohos_runtime_base_contract(),
    );
    #[cfg(target_env = "ohos")]
    details.extend(contract_details);
    #[cfg(target_env = "ohos")]
    {
        let profile = codex_uds::ohos_runtime_profile_contract();
        details.push(format!("runtime directory build profile: {profile}"));
        if profile == "hdc-debug" {
            details.push("runtime directory deployment: HDC shell UID 2000 only; trusts platform system and shell-group services at fixed ancestors; not a commercial PC default and does not enable command isolation".to_owned());
        } else if profile == "platform" {
            let identity = codex_uds::OhosProcessIdentity::current();
            details.push(format!("runtime identity: uid={} euid={} gid={} egid={}; no installation-specific UID binding", identity.uid, identity.euid, identity.gid, identity.egid));
            details.push("runtime directory deployment: native Context first, otherwise the fixed platform namespace candidate; missing or unprotected roots fail closed; HiShell acceptance pending".to_owned());
        }
    }
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
fn runtime_contract(profile: &str, base: Option<&str>) -> (CheckStatus, Vec<String>) {
    if profile == "platform" {
        #[cfg(unix)]
        let candidate = codex_uds::ohos_platform_files_candidate();
        #[cfg(not(unix))]
        let candidate = "unavailable on this platform";
        return (if base.is_some() { CheckStatus::Fail } else { CheckStatus::Warning }, vec![
            "runtime directory contract: platform; runtime-base overrides are forbidden".to_owned(),
            format!("platform files candidate: {candidate}; actual source and directory protection not probed"),
            "runtime directory selection: native ApplicationContext or validated fixed platform namespace; no HOME/CODEX_HOME/TMPDIR override".to_owned(),
            "runtime directory validation: commercial HarmonyOS PC acceptance pending; this read-only report does not initialize or approve directories".to_owned(),
            "runtime layout: files/codex/state, files/codex/r/a, files/codex/r/s, files/codex/tmp, files/codex/logs; full control socket names and peer identity checks retained".to_owned(),
            "runtime path initialization: codex doctor --initialize-data-directories --json; skips config/auth and aliases, creates only validated private child directories; --version does not initialize".to_owned(),
        ]);
    }
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
    let (missing, missing_details) = runtime_contract("strict", None);
    assert_eq!(missing, CheckStatus::Fail);
    assert!(
        missing_details
            .join("\n")
            .contains("normal startup is blocked")
    );
    let (bound, details) = runtime_contract("strict", Some("/fixed/fixture"));
    assert_eq!(bound, CheckStatus::Warning);
    let details = details.join("\n");
    assert!(details.contains("CODEX_OHOS_RUNTIME_BASE=/fixed/fixture"));
    assert!(details.contains("acceptance pending"));
    assert!(details.contains("no HOME/CODEX_HOME/TMPDIR/Context fallback"));
    let (status, details) = runtime_contract("platform", None);
    assert_eq!(status, CheckStatus::Warning);
    let details = details.join("\n");
    assert!(details.contains("actual source and directory protection not probed"));
    assert!(details.contains("--initialize-data-directories --json"));
    assert!(details.contains("--version does not initialize"));
    assert_eq!(
        runtime_contract("platform", Some("/override")).0,
        CheckStatus::Fail
    );
}
