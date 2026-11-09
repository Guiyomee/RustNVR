//! Memory of the built-in recognition, as RustNVR uses it: the resident
//! memory (VmRSS) after analyses run all on one thread (`one`), on a new
//! thread each (`threads`), three at a time on Tokio's blocking pool
//! (`pool`, as RustNVR did until 2026.11.04), or three at a time through
//! RustNVR's own path (`shared`: one analysis thread).
//! `cargo run --release --example onnx_memory --features onnx -- model.onnx picture.jpg [one|threads|pool|shared]`

use rustnvr_ai::ObjectDetector;
use rustnvr_ai::onnx::{OnnxDetector, SharedOnnx};
use std::sync::Arc;

fn rss_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1).map(str::to_owned))
        })
        .and_then(|kb| kb.parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: onnx_memory MODEL.onnx PICTURE.jpg [one|threads|pool|shared]";
    let model = args.next().ok_or(usage)?;
    let picture = args.next().ok_or(usage)?;
    let mode = args.next().unwrap_or_else(|| "threads".to_owned());
    let jpeg = Arc::new(std::fs::read(&picture).map_err(|e| format!("{picture}: {e}"))?);
    println!("start: {:.0} MiB", rss_mib());
    if mode == "shared" {
        let shared = Arc::new(SharedOnnx::new(OnnxDetector::load(model.as_ref())?)?);
        println!("model loaded: {:.0} MiB", rss_mib());
        let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        for round in 1..=12 {
            rt.block_on(async {
                let jobs: Vec<_> = (0..3)
                    .map(|_| {
                        let shared = shared.clone();
                        let jpeg = jpeg.clone();
                        tokio::spawn(async move { shared.detect(&jpeg).await.map(|_| ()) })
                    })
                    .collect();
                for j in jobs {
                    let _ = j.await;
                }
            });
            if round == 1 || round % 3 == 0 {
                println!(
                    "after {} analyses (shared, 3 asked at a time): {:.0} MiB",
                    round * 3,
                    rss_mib()
                );
            }
        }
        return Ok(());
    }
    let detector = Arc::new(OnnxDetector::load(model.as_ref())?);
    println!("model loaded: {:.0} MiB", rss_mib());
    if mode == "pool" {
        let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        for round in 1..=12 {
            rt.block_on(async {
                let jobs: Vec<_> = (0..3)
                    .map(|_| {
                        let detector = detector.clone();
                        let jpeg = jpeg.clone();
                        tokio::task::spawn_blocking(move || {
                            detector.detect_blocking(&jpeg).map(|_| ())
                        })
                    })
                    .collect();
                for j in jobs {
                    let _ = j.await;
                }
            });
            if round == 1 || round % 3 == 0 {
                println!(
                    "after {} analyses (pool, 3 at a time): {:.0} MiB",
                    round * 3,
                    rss_mib()
                );
            }
        }
        return Ok(());
    }
    for i in 1..=24 {
        let run = {
            let detector = detector.clone();
            let jpeg = jpeg.clone();
            move || {
                detector
                    .detect_blocking(&jpeg)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
        };
        if mode == "one" {
            run()?;
        } else {
            // A new thread each time, as a blocking pool hands analyses out.
            std::thread::spawn(run)
                .join()
                .map_err(|_| "thread panicked")??;
        }
        if i == 1 || i % 6 == 0 {
            println!("after {i} analyses ({mode}): {:.0} MiB", rss_mib());
        }
    }
    Ok(())
}
