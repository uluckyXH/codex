//! Opens TUI links using a supported host backend, or leaves the URL visible for manual opening.

#[cfg(not(target_env = "ohos"))]
pub(crate) fn open(url: &str) -> std::io::Result<()> {
    webbrowser::open(url)
}

#[cfg(target_env = "ohos")]
pub(crate) fn open(url: &str) -> std::io::Result<()> {
    // API 24's public C Ability surface starts self UIAbilities with application
    // context. A standalone CLI has no such context or general open-URL API.
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!(
            "automatic browser opening is unavailable on HarmonyOS: {}; open this URL manually: {url}",
            codex_features::platform::BROWSER_OPEN_UNAVAILABLE
        ),
    ))
}
