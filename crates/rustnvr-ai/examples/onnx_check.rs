//! Runs the built-in recognition on JPEG files and prints what it finds,
//! with the time taken: `cargo run --release --example onnx_check --features
//! onnx -- model.onnx picture.jpg…`

use rustnvr_ai::onnx::OnnxDetector;
use std::time::Instant;

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .ok_or("usage: onnx_check MODEL.onnx PICTURE.jpg…")?;
    let started = Instant::now();
    let detector = OnnxDetector::load(model.as_ref())?;
    println!(
        "model loaded in {} ms (input {}×{})",
        started.elapsed().as_millis(),
        detector.input_size(),
        detector.input_size()
    );
    let mut times = Vec::new();
    for path in args {
        let jpeg = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
        let started = Instant::now();
        let found = detector.detect_blocking(&jpeg).map_err(|e| e.to_string())?;
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        times.push(ms);
        let labels: Vec<String> = found
            .iter()
            .filter(|o| o.score >= 0.4)
            .map(|o| {
                let b = o.bbox.unwrap_or_default();
                format!(
                    "{} {:.2} [{:.2},{:.2},{:.2},{:.2}]",
                    o.label, o.score, b[0], b[1], b[2], b[3]
                )
            })
            .collect();
        let name = path.rsplit(['/', '\\']).next().unwrap_or(&path);
        println!("{name} {ms:.0} ms: {}", labels.join(", "));
    }
    if !times.is_empty() {
        times.sort_by(f64::total_cmp);
        println!(
            "{} pictures: min {:.0} ms, median {:.0} ms, max {:.0} ms",
            times.len(),
            times[0],
            times[times.len() / 2],
            times[times.len() - 1]
        );
    }
    Ok(())
}
