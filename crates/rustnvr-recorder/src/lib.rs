//! Continuous recording of compressed video into fragmented MP4 segments.
//!
//! The recording path copies the encoded bitstream received from the camera:
//! nothing is decoded or re-encoded. See ADR 0001 for the container choice.
//!
//! * [`fmp4`]: ISO BMFF box writer (initialization segment and fragments);
//! * [`scan`]: fragment-level parser used by crash recovery and tests;
//! * [`store`]: segment files (`.mp4.part` → fsync → rename to `.mp4`);
//! * [`recorder`]: synchronous segmentation state machine;
//! * [`driver`]: runs a recorder on a dedicated OS thread;
//! * [`recovery`]: repairs segments left behind by a crash.

pub mod destination;
pub mod driver;
pub mod fmp4;
pub mod gate;
pub mod live_mux;
pub mod recorder;
pub mod recovery;
pub mod retention;
pub mod scan;
pub mod store;

pub use destination::{Destination, DestinationStore};
pub use driver::{RecorderHandle, spawn_recorder};
pub use gate::{MotionGate, RecordTrigger};
pub use recorder::{RecorderCore, RecorderMetrics, RecorderSettings};
pub use recovery::{RecoveryReport, recover_part_file, recover_tree};
pub use retention::{Protected, RetentionPolicy, RetentionReport};
pub use store::{FsStore, SegmentFile, SegmentStore};
