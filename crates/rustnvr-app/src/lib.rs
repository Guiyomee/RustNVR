//! Assembly of the RustNVR components into a running NVR.
//!
//! The binary (`src/main.rs`) only parses the command line; everything that
//! integration tests need is exposed here.

pub mod ai_store;
pub mod backup;
pub mod camera_store;
pub mod captures;
pub mod logging;
pub mod mqtt_store;
pub mod nvr;
pub mod storage_store;

pub use nvr::Nvr;
