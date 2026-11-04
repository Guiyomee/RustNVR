//! Events published by NVR components.
//!
//! The [`EventBus`] is a lossy broadcast channel intended for notifications
//! (logs, MQTT, WebSocket). Components whose correctness depends on seeing
//! every event must not rely on it; for example the future SQLite index will
//! be rebuildable from the recordings on disk.

use crate::camera::{CameraId, StreamRole};
use crate::detection::DetectionEvent;
use crate::media::VideoCodec;
use bytes::Bytes;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use tokio::sync::broadcast;

/// Connection state of one camera stream.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamState {
    /// The supervisor has not started yet.
    Idle,
    /// Connecting (DESCRIBE/SETUP/PLAY in progress).
    Connecting {
        /// Consecutive attempt number since the last stable connection, from 1.
        attempt: u32,
    },
    /// Frames are being received.
    Streaming {
        /// Session identifier.
        session: u64,
        /// Codec of the stream.
        codec: VideoCodec,
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// Audio received with the video (`mp4a.40.2 16000 Hz`), if any.
        audio: Option<String>,
        /// When the first frame of the session was received.
        since: SystemTime,
    },
    /// The last attempt failed or the session was lost; waiting before retrying.
    Retrying {
        /// Consecutive attempt number that failed.
        attempt: u32,
        /// Delay before the next attempt.
        retry_in: Duration,
        /// One-line description of the failure (never contains credentials).
        error: String,
    },
    /// The supervisor stopped (shutdown).
    Stopped,
}

impl StreamState {
    /// Whether frames are currently flowing.
    pub fn is_streaming(&self) -> bool {
        matches!(self, StreamState::Streaming { .. })
    }
}

/// A finished recording segment.
#[derive(Clone, Debug, PartialEq)]
pub struct SegmentInfo {
    /// Camera the segment belongs to.
    pub camera: CameraId,
    /// Final path of the segment file.
    pub path: PathBuf,
    /// Wall-clock time of the first frame.
    pub start: SystemTime,
    /// Media duration, computed from frame timestamps.
    pub duration: Duration,
    /// File size in bytes.
    pub bytes: u64,
    /// Number of frames.
    pub frames: u64,
    /// Number of fMP4 fragments.
    pub fragments: u32,
    /// Codec.
    pub codec: VideoCodec,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Notifications published on the [`EventBus`].
#[derive(Clone, Debug, PartialEq)]
pub enum NvrEvent {
    /// A stream changed state.
    StreamState {
        /// Camera.
        camera: CameraId,
        /// Which stream of the camera.
        role: StreamRole,
        /// New state.
        state: StreamState,
    },
    /// A recording segment was completed and renamed into place.
    SegmentFinished(SegmentInfo),
    /// Cameras were added, changed or removed at run time.
    CamerasChanged,
    /// The recorder failed to write and will retry later.
    RecorderError {
        /// Camera.
        camera: CameraId,
        /// Description of the failure.
        message: String,
        /// Partial segment kept after truncation to its last complete fragment, if any.
        salvaged: Option<PathBuf>,
    },
    /// A detection event started, changed (thumbnail, zones) or ended.
    Detection(DetectionEvent),
    /// A picture taken when an event started (JPEG), for integrations.
    Snapshot {
        /// Camera.
        camera: CameraId,
        /// JPEG picture.
        jpeg: Bytes,
    },
    /// The picture where an object was recognized, with the boxes of the
    /// objects of that kind drawn on it (JPEG), for integrations.
    ObjectSnapshot {
        /// Camera.
        camera: CameraId,
        /// Kind of object (`person`, `car`…).
        label: String,
        /// JPEG picture.
        jpeg: Bytes,
    },
    /// What moves crossed a line drawn on the camera.
    Crossing {
        /// Camera.
        camera: CameraId,
        /// Name of the line.
        line: String,
        /// From the left of the line (as drawn) to its right.
        forward: bool,
        /// When.
        at_ms: i64,
        /// Crossings of this line since local midnight, this one included.
        today: u64,
    },
    /// Motion detection was switched on or off for a camera.
    MotionSettingsChanged {
        /// Camera.
        camera: CameraId,
        /// Whether detection is now enabled.
        enabled: bool,
    },
}

/// Lossy, bounded, multi-consumer event channel.
pub type EventBus = broadcast::Sender<NvrEvent>;

/// Creates an [`EventBus`] keeping at most `capacity` undelivered events per subscriber.
pub fn event_bus(capacity: usize) -> EventBus {
    broadcast::channel(capacity).0
}
