//! Runs YOLOX with ONNX Runtime (the `ort` crate) instead of tract, on JPEG
//! files, to compare times: `cargo run --release --example ort_trial
//! --features onnx -- MODEL.onnx PICTURE.jpg…` (input 640, as YOLOX-s/m).

use ort::session::Session;
use ort::value::Tensor;
use std::time::Instant;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

const SIZE: usize = 640;

/// The picture scaled into SIZE×SIZE, BGR planes 0–255, padded with 114,
/// as RustNVR feeds YOLOX; returns it and the scale.
fn letterbox(jpeg: &[u8]) -> Result<(Vec<f32>, f32), String> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(jpeg), options);
    let px = decoder.decode().map_err(|e| format!("JPEG: {e:?}"))?;
    let (width, height) = decoder.dimensions().ok_or("JPEG without dimensions")?;
    let ratio = (SIZE as f32 / width as f32).min(SIZE as f32 / height as f32);
    let (w, h) = (
        ((width as f32 * ratio).round() as usize).clamp(1, SIZE),
        ((height as f32 * ratio).round() as usize).clamp(1, SIZE),
    );
    let plane = SIZE * SIZE;
    let mut out = vec![114f32; 3 * plane];
    let at = |x: usize, y: usize, c: usize| f32::from(px[(y * width + x) * 3 + c]);
    for y in 0..h {
        let sy = ((y as f32 + 0.5) / ratio - 0.5).clamp(0.0, (height - 1) as f32);
        let (y0, fy) = (sy.floor() as usize, sy.fract());
        let y1 = (y0 + 1).min(height - 1);
        for x in 0..w {
            let sx = ((x as f32 + 0.5) / ratio - 0.5).clamp(0.0, (width - 1) as f32);
            let (x0, fx) = (sx.floor() as usize, sx.fract());
            let x1 = (x0 + 1).min(width - 1);
            for (p, c) in [2, 1, 0].into_iter().enumerate() {
                let top = at(x0, y0, c) * (1.0 - fx) + at(x1, y0, c) * fx;
                let bottom = at(x0, y1, c) * (1.0 - fx) + at(x1, y1, c) * fx;
                out[p * plane + y * SIZE + x] = top * (1.0 - fy) + bottom * fy;
            }
        }
    }
    Ok((out, ratio))
}

/// Best person score and best other score (class index) in YOLOX rows.
fn best(rows: &[f32]) -> (f32, usize, f32) {
    let (mut person, mut other, mut class) = (0f32, 0f32, 0usize);
    for r in rows.as_chunks::<85>().0 {
        let (c, s) =
            r[5..85].iter().enumerate().fold(
                (0, f32::MIN),
                |a, (i, s)| if *s > a.1 { (i, *s) } else { a },
            );
        let score = r[4] * s;
        if c == 0 {
            person = person.max(score);
        } else if score > other {
            other = score;
            class = c;
        }
    }
    (person, class, other)
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: ort_trial libonnxruntime.so MODEL.onnx PICTURE.jpg…";
    let lib = args.next().ok_or(usage)?;
    let model = args.next().ok_or(usage)?;
    let started = Instant::now();
    let env = ort::init_from(&lib)
        .map_err(|e| format!("cannot load {lib}: {e}"))?
        .build()
        .map_err(|e| e.to_string())?;
    println!("{lib}: loaded in {} ms", started.elapsed().as_millis());
    let started = Instant::now();
    let mut session = Session::builder(&env)
        .map_err(|e| e.to_string())?
        .commit_from_file(&model)
        .map_err(|e| format!("cannot load {model}: {e}"))?;
    println!(
        "ort {model}: loaded in {} ms",
        started.elapsed().as_millis()
    );
    let mut times = Vec::new();
    for picture in args {
        let jpeg = std::fs::read(&picture).map_err(|e| format!("{picture}: {e}"))?;
        let started = Instant::now();
        let (input, _) = letterbox(&jpeg)?;
        let tensor = Tensor::from_array(([1usize, 3, SIZE, SIZE], input.into_boxed_slice()))
            .map_err(|e| e.to_string())?;
        let outputs = session
            .run(ort::inputs![tensor])
            .map_err(|e| format!("inference: {e}"))?;
        let (_, rows) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        let (person, class, other) = best(rows);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        times.push(ms);
        let name = picture.rsplit(['/', '\\']).next().unwrap_or(&picture);
        println!("  {name} {ms:.0} ms: person {person:.2}, best other class {class} {other:.2}");
    }
    if !times.is_empty() {
        times.sort_by(f64::total_cmp);
        println!(
            "  {} pictures: median {:.0} ms (min {:.0}, max {:.0})",
            times.len(),
            times[times.len() / 2],
            times[0],
            times[times.len() - 1]
        );
    }
    Ok(())
}
