//! RTSP republication: other programs (Home Assistant, Frigate, VLC…) read
//! the cameras from RustNVR instead of opening their own connection to them.
//!
//! The server takes the frames of a [`rustnvr_stream::LiveTap`] (the ones
//! already received for recording and the live view) and sends them as RTP,
//! without decoding or re-encoding:
//!
//! * [`describe`]: what a stream carries, and its SDP description;
//! * [`rtp`]: RTP packets of H.264, H.265, AAC and Opus;
//! * [`server`]: the RTSP/1.0 server (RTP interleaved in the RTSP connection
//!   only, no authentication).
//!
//! Nothing runs while no client is connected.

pub mod describe;
pub mod rtp;
pub mod server;

pub use server::{Lookup, RestreamServer};
