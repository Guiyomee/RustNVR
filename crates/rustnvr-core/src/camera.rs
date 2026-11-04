//! Camera identifiers and stream roles.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Maximum length of a camera identifier.
pub const MAX_CAMERA_ID_LEN: usize = 64;

/// Stable identifier of a camera.
///
/// Identifiers are used in file paths, and later in MQTT topics and Home
/// Assistant `unique_id`s, so they are restricted to `[a-z0-9_-]`, must start
/// with a letter or digit and are at most [`MAX_CAMERA_ID_LEN`] characters.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CameraId(String);

/// Error returned for an identifier that does not follow the rules of [`CameraId`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid camera id {0:?}: expected 1-{MAX_CAMERA_ID_LEN} characters in [a-z0-9_-], starting with a letter or digit"
)]
pub struct InvalidCameraId(pub String);

impl CameraId {
    /// Validates and wraps an identifier.
    pub fn new(id: impl Into<String>) -> Result<Self, InvalidCameraId> {
        let id = id.into();
        let valid_chars = id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        let valid_start = id
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        if valid_chars && valid_start && id.len() <= MAX_CAMERA_ID_LEN {
            Ok(Self(id))
        } else {
            Err(InvalidCameraId(id))
        }
    }

    /// Returns the identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CameraId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for CameraId {
    type Err = InvalidCameraId;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for CameraId {
    type Error = InvalidCameraId;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl From<CameraId> for String {
    fn from(id: CameraId) -> Self {
        id.0
    }
}

/// Which of a camera's streams a component works with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamRole {
    /// Full resolution stream, used for recording.
    Main,
    /// Low resolution stream, intended for detection and live thumbnails.
    Sub,
}

impl fmt::Display for StreamRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StreamRole::Main => "main",
            StreamRole::Sub => "sub",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_ids() {
        for id in ["garage", "cam-1", "front_door", "0cam"] {
            assert!(CameraId::new(id).is_ok(), "{id}");
        }
    }

    #[test]
    fn rejects_invalid_ids() {
        let too_long = "a".repeat(MAX_CAMERA_ID_LEN + 1);
        for id in [
            "", "Garage", "cam 1", "-cam", "_cam", "cam/1", "../x", "café", &too_long,
        ] {
            assert!(CameraId::new(id).is_err(), "{id}");
        }
    }
}
