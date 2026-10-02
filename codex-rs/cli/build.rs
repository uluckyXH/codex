fn main() {
    println!("cargo:rerun-if-env-changed=CODEX_HARMONY_BUILD_ID");
    if let Ok(target) = std::env::var("TARGET") {
        println!("cargo:rustc-env=CODEX_CLI_BUILD_TARGET={target}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-ObjC");
    }
}
