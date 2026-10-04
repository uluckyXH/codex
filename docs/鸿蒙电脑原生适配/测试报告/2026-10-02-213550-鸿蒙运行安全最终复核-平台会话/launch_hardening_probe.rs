#![allow(dead_code)]
// Compile the actual implementation as a module; no copied hardening logic.
#[path = "../../../../codex-rs/process-hardening/src/lib.rs"]
mod production;

fn main() {
    #[cfg(target_os = "macos")]
    let loader_key = "DYLD_CODEX_HARDENING_PROBE";
    #[cfg(not(target_os = "macos"))]
    let loader_key = "LD_CODEX_HARDENING_PROBE";
    // SAFETY: this standalone process has not created any threads. The caller
    // provides an empty environment except PATH and this batch's TMPDIR.
    unsafe {
        std::env::set_var(loader_key, "probe-only");
        std::env::set_var("CODEX_PROBE_KEPT", "probe-only");
    }
    production::pre_main_hardening();
    assert!(std::env::var_os(loader_key).is_none());
    assert_eq!(std::env::var("CODEX_PROBE_KEPT").as_deref(), Ok("probe-only"));
    let mut limit = libc::rlimit { rlim_cur: 1, rlim_max: 1 };
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_CORE, &mut limit) }, 0);
    assert_eq!((limit.rlim_cur, limit.rlim_max), (0, 0));
    #[cfg(target_os = "linux")]
    assert_eq!(unsafe { libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0) }, 0);
    println!("loader marker removed; unrelated marker retained; core limits zero");
}
