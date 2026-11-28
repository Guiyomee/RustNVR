//! Plate reading on JPEG files, as RustNVR does it: YOLOX finds the
//! vehicles, then the plate models read them (the whole picture when no
//! vehicle is found). `RUSTNVR_ORT_LIB=off` forces tract.
//!
//! `cargo run --release --example plate_check --features onnx -- yolox.onnx
//! plate_detector.onnx plate_reader.onnx picture.jpg…`

use rustnvr_ai::onnx::OnnxDetector;
use rustnvr_ai::plates::PlateReader;
use std::time::Instant;

/// Labels whose plate is read.
const VEHICLES: [&str; 4] = ["car", "truck", "bus", "motorcycle"];

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: plate_check YOLOX.onnx DETECTOR.onnx READER.onnx PICTURE.jpg…";
    let yolox = OnnxDetector::load(args.next().ok_or(usage)?.as_ref())?;
    let detector = args.next().ok_or(usage)?;
    let reader = args.next().ok_or(usage)?;
    let started = Instant::now();
    let plates = PlateReader::load(detector.as_ref(), reader.as_ref())?;
    println!(
        "plate models loaded in {} ms ({})",
        started.elapsed().as_millis(),
        plates.engine()
    );
    let (mut read, mut vehicles_seen, mut times) = (0, 0, Vec::new());
    for path in args {
        let jpeg = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
        let found = yolox.detect_blocking(&jpeg).map_err(|e| e.to_string())?;
        let mut boxes: Vec<[f32; 4]> = found
            .iter()
            .filter(|o| o.score >= 0.4 && VEHICLES.contains(&o.label.as_str()))
            .filter_map(|o| o.bbox)
            .take(4)
            .collect();
        if boxes.is_empty() {
            boxes.push([0.0, 0.0, 1.0, 1.0]);
        }
        vehicles_seen += boxes.len();
        let started = Instant::now();
        let result = plates
            .read_blocking(&jpeg, &boxes)
            .map_err(|e| e.to_string())?;
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        times.push(ms);
        let texts: Vec<String> = result
            .iter()
            .map(|p| match p {
                Some(p) => format!("{} ({:.2}, {} px)", p.text, p.confidence, p.width),
                None => "-".to_owned(),
            })
            .collect();
        read += result.iter().flatten().count();
        let name = path.rsplit(['/', '\\']).next().unwrap_or(&path);
        println!("{name} {ms:.0} ms: {}", texts.join(", "));
    }
    if !times.is_empty() {
        times.sort_by(f64::total_cmp);
        println!(
            "{read} plates read on {vehicles_seen} vehicles, {} pictures: median {:.0} ms, max {:.0} ms",
            times.len(),
            times[times.len() / 2],
            times[times.len() - 1]
        );
    }
    Ok(())
}
