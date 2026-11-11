//! RTSP republication: other programs (Home Assistant, Frigate, VLC…) read
//! the cameras from RustNVR instead of opening their own connection to them.
//!
//! The server takes the frames of a [`rustnvr_stream::LiveTap`] (the ones
//! already received for recording and the live view) and sends them as RTP,
//! without decoding or re-encoding:
//!
//! * [`auth`]: Digest and Basic authentication of the clients;
//! * [`describe`]: what a stream carries, and its SDP description;
//! * [`rtp`]: RTP packets of H.264, H.265, AAC and Opus;
//! * [`server`]: the RTSP/1.0 server (RTP interleaved in the RTSP connection
//!   only).
//!
//! Nothing runs while no client is connected.

pub mod auth;
pub mod describe;
pub mod rtp;
pub mod server;

pub use auth::Credentials;
pub use server::{Lookup, Options, RestreamServer, Source};
