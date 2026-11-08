//! Shared building blocks of RustNVR.
//!
//! This crate deliberately has no knowledge of RTSP libraries, file formats or
//! network services: it defines the vocabulary (identifiers, configuration,
//! media frames, events) that the other crates exchange, so that ingestion,
//! recording, detection and integrations stay independent of each other.

pub mod camera;
pub mod config;
pub mod detection;
pub mod events;
pub mod media;
pub mod rtsp;
pub mod secret;

pub use camera::{CameraId, InvalidCameraId, StreamRole};
pub use config::{Config, ConfigError};
pub use detection::{
    CrossingDirection, CrossingLine, DetectedObject, DetectionEvent, EventKind, MotionSettings,
    MotionZone, PrivacyMask, presence,
};
pub use events::{EventBus, NvrEvent, SegmentInfo, StreamState};
pub use media::{
    AudioParams, AudioSample, FrameAudio, StreamItem, VideoCodec, VideoFrame, VideoParams,
};
pub use rtsp::{Credentials, RtspTarget, RtspTargetError};
pub use secret::Secret;

/// The running version as releases are named (`2026.11.03`). Cargo's
/// version cannot have a leading zero (`2026.11.3`); the release number is
/// shown on two digits like the tags.
pub fn release_version() -> &'static str {
    static VERSION: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| release_name(env!("CARGO_PKG_VERSION")));
    &VERSION
}

/// `2026.11.3` → `2026.11.03` (also with a pre-release suffix); other
/// versions unchanged.
fn release_name(version: &str) -> String {
    let (numbers, suffix) = match version.find(['-', '+']) {
        Some(i) => version.split_at(i),
        None => (version, ""),
    };
    match numbers.split('.').collect::<Vec<_>>().as_slice() {
        [year, month, number]
            if year.len() == 4
                && number.len() == 1
                && number.bytes().all(|b| b.is_ascii_digit()) =>
        {
            format!("{year}.{month}.0{number}{suffix}")
        }
        _ => version.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_are_named_like_their_tags() {
        assert_eq!(release_name("2026.11.3"), "2026.11.03");
        assert_eq!(release_name("2026.11.12"), "2026.11.12");
        assert_eq!(release_name("2026.11.4-rc1"), "2026.11.04-rc1");
        assert_eq!(release_name("0.2.1"), "0.2.1");
        assert!(release_version().starts_with("20"));
        assert!(!config::is_newer_version(
            release_version(),
            env!("CARGO_PKG_VERSION")
        ));
        assert!(!config::is_newer_version(
            env!("CARGO_PKG_VERSION"),
            release_version()
        ));
    }
}
