mod client;
mod helper_exit;
#[cfg(any(target_os = "linux", test))]
mod linux_alsa;
mod message_reader;
mod protocol;
mod session;

fn ensure_native_voice_supported() -> anyhow::Result<()> {
    if cfg!(target_env = "ohos") {
        anyhow::bail!(
            "native voice capture and playback are unavailable on HarmonyOS; no compatible voice host is packaged"
        );
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn native_voice_gate_matches_the_target_without_starting_a_host() {
    assert_eq!(
        ensure_native_voice_supported().is_err(),
        cfg!(target_env = "ohos")
    );
}

pub use client::ConnectionError;
pub use client::VoiceHost;
pub use helper_exit::HelperExitStage;
pub use protocol::AudioControls;
pub use protocol::AudioDevice;
pub use protocol::AudioDeviceKind;
pub use protocol::AudioDeviceSelection;
pub use protocol::AudioState;
pub use protocol::MAX_FRAME_BYTES;
pub use protocol::Message;
pub use protocol::RUNTIME_ENVIRONMENT;
pub use protocol::SessionDescription;
pub use protocol::decode_frame;
pub use protocol::encode_frame;
pub use protocol::read_message;
pub use session::RealtimeWebrtcSession;
pub use session::RealtimeWebrtcSessionHandle;
pub use session::StartedRealtimeWebrtcSession;
