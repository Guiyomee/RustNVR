//! RTSP ingestion for RustNVR.
//!
//! * [`session`]: one RTSP session (DESCRIBE/SETUP/PLAY with timeouts) built
//!   on [Retina](https://crates.io/crates/retina), producing
//!   [`rustnvr_core::VideoFrame`]s without decoding;
//! * [`supervisor`]: one long-lived task per stream that reconnects with
//!   exponential backoff, detects stalls and publishes its state;
//! * [`fanout`]: bounded, non-blocking distribution of frames to consumers;
//! * [`live`]: on-demand access to the frames for live viewers;
//! * [`probe`](mod@probe): connection test used by the CLI (and later the API).
//!
//! Retina types do not leave this crate.

pub mod backoff;
pub mod error;
pub mod fanout;
pub mod live;
pub mod metrics;
pub mod probe;
pub mod session;
pub mod supervisor;

pub use backoff::{Backoff, BackoffConfig};
pub use error::StreamError;
pub use fanout::{FrameFanout, SubscriberStats};
pub use live::LiveTap;
pub use metrics::{StreamMetrics, StreamMetricsSnapshot};
pub use probe::{ProbeReport, ProbedStream, probe};
pub use session::{RtspSession, SessionOptions};
pub use supervisor::{StreamHandle, SupervisorConfig, spawn_supervisor};
