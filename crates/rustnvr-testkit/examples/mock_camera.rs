//! A mock camera for trying RustNVR by hand: serves the H.264 (or H.265)
//! fixture in real time, with a G.711 tone, until stopped.
//! `cargo run -p rustnvr-testkit --example mock_camera [h265]`
//! then a camera with the printed URL in RustNVR.

use rustnvr_testkit::{Fixture, MockRtspServer, MockServerConfig};
use std::time::Duration;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let h265 = std::env::args().nth(1).as_deref() == Some("h265");
    let fixture = if h265 {
        Fixture::h265()
    } else {
        Fixture::h264()
    };
    let mut config = MockServerConfig::new(fixture);
    config.frame_interval = Duration::from_millis(100); // 10 fps, real time
    config.pcmu_audio = true;
    let server = MockRtspServer::start(config).await?;
    println!("{}", server.url());
    tokio::signal::ctrl_c().await?;
    server.stop().await;
    Ok(())
}
