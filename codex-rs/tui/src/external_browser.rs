//! Opens non-authentication links from the TUI using a supported host backend.

#[cfg(not(target_env = "ohos"))]
pub(crate) fn open(url: &str) -> std::io::Result<()> {
    webbrowser::open(url)
}

#[cfg(target_env = "ohos")]
pub(crate) fn open(url: &str) -> std::io::Result<()> {
    // The Linux launcher probes xdg-open and desktop-specific commands. A native
    // HarmonyOS launcher has not been integrated, so keep the URL visible instead.
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!(
            "automatic browser opening is unavailable on HarmonyOS; open this URL manually: {url}"
        ),
    ))
}
