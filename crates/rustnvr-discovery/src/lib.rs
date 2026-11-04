//! Finding cameras on the local network, to add them without typing their
//! RTSP address:
//!
//! - [`wsd`]: ONVIF WS-Discovery (cameras answer a multicast probe);
//! - [`scan`]: RTSP port scan of a network (cameras without ONVIF, or when
//!   multicast does not reach RustNVR);
//! - [`onvif`]: with the camera's user name and password, its maker, model
//!   and the RTSP address of each of its streams;
//! - [`paths`]: usual RTSP paths of common makers, for cameras without ONVIF.
//!
//! Nothing here is stored: the interface prefills the camera form.

pub mod onvif;
pub mod paths;
pub mod scan;
pub mod wsd;
mod xml;
