// 直接引用产品的无依赖启动策略；目标条件由实际 Rust triple 决定。
#[path = "/Volumes/MacSSD/Repositories/codex/.harmony-build/工作副本/终端适配/codex-rs/tui/src/daemon_startup/platform.rs"]
mod platform;

#[cfg(target_env = "ohos")]
const _: () = assert!(!platform::local_daemon_supported());
#[cfg(not(target_env = "ohos"))]
const _: () = assert!(platform::local_daemon_supported());
