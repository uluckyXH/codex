use std::env;
use std::path::Path;
use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(bwrap_available)");
    println!("cargo:rerun-if-env-changed=CODEX_BWRAP_SOURCE_DIR");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_ALLOW_CROSS");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_SYSROOT_DIR");
    println!("cargo:rerun-if-env-changed=CODEX_SKIP_BWRAP_BUILD");
    println!("cargo:rerun-if-env-changed=CODEX_OHOS_LIBCAP_DIR");
    println!("cargo:rerun-if-changed=ohos_compat.h");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let vendor_dir = manifest_dir.join("../vendor/bubblewrap");
    for source in ["bubblewrap.c", "bind-mount.c", "network.c", "utils.c"] {
        println!(
            "cargo:rerun-if-changed={}",
            vendor_dir.join(source).display()
        );
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "linux" || env::var_os("CODEX_SKIP_BWRAP_BUILD").is_some() {
        return;
    }

    if let Err(err) = try_build_bwrap() {
        panic!("failed to compile bubblewrap for Linux target: {err}");
    }
}

fn try_build_bwrap() -> Result<(), String> {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").map_err(|err| err.to_string())?);
    let out_dir = PathBuf::from(env::var("OUT_DIR").map_err(|err| err.to_string())?);
    let src_dir = resolve_bwrap_source_dir(&manifest_dir)?;
    let is_ohos = env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("ohos");
    let (include_paths, link_paths, libs) = if is_ohos {
        // A target-only prefix avoids both a host pkg-config dependency and
        // accidental selection of a Homebrew/Linux libcap for the OHOS ELF.
        let prefix = PathBuf::from(env::var("CODEX_OHOS_LIBCAP_DIR").map_err(|_| {
            "OHOS requires CODEX_OHOS_LIBCAP_DIR; build libcap with scripts/build_harmony_helpers.py and pass --native-deps to scripts/build_harmony.py".to_string()
        })?);
        if !prefix.is_absolute()
            || !prefix.join("include/sys/capability.h").is_file()
            || !prefix.join("lib/libcap.a").is_file()
        {
            return Err(format!("invalid OHOS libcap prefix: {}", prefix.display()));
        }
        (
            vec![prefix.join("include")],
            vec![prefix.join("lib")],
            vec!["cap".to_string()],
        )
    } else {
        let libcap = pkg_config::Config::new()
            .cargo_metadata(false)
            .probe("libcap")
            .map_err(|err| format!("libcap not available via pkg-config: {err}"))?;
        (libcap.include_paths, libcap.link_paths, libcap.libs)
    };

    let config_h = out_dir.join("config.h");
    let mut config = r#"#pragma once
#define PACKAGE_STRING "bubblewrap built for Codex"
"#
    .to_string();
    if is_ohos {
        config.push_str("#include \"ohos_compat.h\"\n");
    }
    std::fs::write(&config_h, config)
        .map_err(|err| format!("failed to write {}: {err}", config_h.display()))?;

    let mut build = cc::Build::new();
    build
        .file(src_dir.join("bubblewrap.c"))
        .file(src_dir.join("bind-mount.c"))
        .file(src_dir.join("network.c"))
        .file(src_dir.join("utils.c"))
        .include(&out_dir)
        .include(&src_dir)
        .include(&manifest_dir)
        .define("_GNU_SOURCE", None)
        // Rename `main` so the Rust wrapper can expose the Cargo-built binary.
        .define("main", Some("bwrap_main"));
    for include_path in include_paths {
        if is_ohos {
            // The SDK's sys/capability.h only declares capget/capset, whereas
            // bubblewrap also needs libcap's cap_value_t and cap_from_name.
            build.include(include_path);
            continue;
        }
        // Use -idirafter so target sysroot headers win (musl cross builds),
        // while still allowing libcap headers from the host toolchain.
        build.flag(format!("-idirafter{}", include_path.display()));
    }

    build.compile("standalone_bwrap");
    for link_path in link_paths {
        println!("cargo:rustc-link-search=native={}", link_path.display());
    }
    for lib in libs {
        if is_ohos {
            println!("cargo:rustc-link-lib=static={lib}");
        } else {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
    println!("cargo:rustc-cfg=bwrap_available");
    Ok(())
}

/// Resolve the bubblewrap source directory used for build-time compilation.
///
/// Priority:
/// 1. `CODEX_BWRAP_SOURCE_DIR` points at an existing bubblewrap checkout.
/// 2. The vendored bubblewrap tree under `codex-rs/vendor/bubblewrap`.
fn resolve_bwrap_source_dir(manifest_dir: &Path) -> Result<PathBuf, String> {
    if let Ok(path) = env::var("CODEX_BWRAP_SOURCE_DIR") {
        let src_dir = PathBuf::from(path);
        if src_dir.exists() {
            return Ok(src_dir);
        }
        return Err(format!(
            "CODEX_BWRAP_SOURCE_DIR was set but does not exist: {}",
            src_dir.display()
        ));
    }

    let vendor_dir = manifest_dir.join("../vendor/bubblewrap");
    if vendor_dir.exists() {
        return Ok(vendor_dir);
    }

    Err(format!(
        "expected vendored bubblewrap at {}, but it was not found.\n\
Set CODEX_BWRAP_SOURCE_DIR to an existing checkout or vendor bubblewrap under codex-rs/vendor.",
        vendor_dir.display()
    ))
}
