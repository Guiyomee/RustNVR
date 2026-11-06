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
