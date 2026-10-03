#![allow(dead_code)]
#[path = "../../../../codex-rs/arg0/src/harmony_alias_dir.rs"]
mod harmony_alias_dir;
#[path = "../../../../codex-rs/arg0/src/harmony_hnp_alias_dir.rs"]
mod harmony_hnp_alias_dir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let aliases = ["apply_patch", "applypatch", "codex-linux-sandbox", "codex-execve-wrapper"];
    let incoming = std::env::args_os().next().unwrap_or_default();
    if let Some(alias) = std::path::Path::new(&incoming).file_name().and_then(|value| value.to_str()) {
        if aliases.contains(&alias) {
            println!("alias-dispatch={alias} forwarded={}", std::env::args().nth(1).as_deref() == Some("HNP_PROBE"));
            return Ok(());
        }
    }
    #[cfg(target_env = "ohos")]
    {
        let executable = std::env::current_exe()?;
        let guard = harmony_alias_dir::prepare(&executable, &aliases)?;
        println!("profile={} uid={} aliases={}", codex_uds::ohos_runtime_profile_contract(), unsafe { libc::geteuid() }, guard.path().display());
        for alias in aliases {
            let output = std::process::Command::new(guard.path().join(alias)).arg("HNP_PROBE").output()?;
            let message = String::from_utf8_lossy(&output.stdout);
            println!("invoke={alias} exit={:?} stdout={} stderr={}", output.status.code(), message.trim(), String::from_utf8_lossy(&output.stderr).trim());
            if !output.status.success() || message.trim() != format!("alias-dispatch={alias} forwarded=true") {
                return Err(format!("alias invocation failed: {alias}").into());
            }
        }
        println!("HNP_ALIAS_VALIDATION_PASS");
    }
    Ok(())
}
