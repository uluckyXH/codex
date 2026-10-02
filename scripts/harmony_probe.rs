//! 最小鸿蒙候选程序：先验证 Rust 标准库和 SDK 链接，再交给真机签名运行。

#[cfg(not(all(target_os = "linux", target_env = "ohos", target_arch = "aarch64")))]
compile_error!("此探针只用于 aarch64-unknown-linux-ohos 目标");

fn main() -> std::io::Result<()> {
    println!("鸿蒙 ARM64 Rust 原生探针启动成功");
    println!("当前目录：{}", std::env::current_dir()?.display());
    println!("程序路径：{}", std::env::current_exe()?.display());
    println!("这只验证基本启动，不代表 Codex、PTY 或沙箱已适配完成。");
    Ok(())
}
