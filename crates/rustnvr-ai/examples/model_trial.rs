//! Tries other object recognition models than YOLOX on JPEG files, with
//! tract, and prints what they find and their time: `cargo run --release
//! --example model_trial --features onnx -- KIND MODEL.onnx PICTURE.jpg…`
//! with KIND `yolox` (as RustNVR runs it), `yolov9` (MultimediaTechLab MIT
//! export: RGB 0–1 stretched to the model size, output `[N, 6]` x0 y0 x1 y1
//! score class with NMS inside) or `rfdetr` (RGB, ImageNet mean/std,
//! stretched; outputs `dets` cxcywh 0–1 and `labels` logits on 91 COCO ids).

use std::time::Instant;
use tract_onnx::prelude::*;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

const COCO: [&str; 80] = [
    "person",
    "bicycle",
    "car",
    "motorcycle",
    "airplane",
    "bus",
    "train",
    "truck",
    "boat",
    "traffic light",
    "fire hydrant",
    "stop sign",
    "parking meter",
    "bench",
    "bird",
    "cat",
    "dog",
    "horse",
    "sheep",
    "cow",
    "elephant",
    "bear",
    "zebra",
    "giraffe",
    "backpack",
    "umbrella",
    "handbag",
    "tie",
    "suitcase",
    "frisbee",
    "skis",
    "snowboard",
    "sports ball",
    "kite",
    "baseball bat",
    "baseball glove",
    "skateboard",
    "surfboard",
    "tennis racket",
    "bottle",
    "wine glass",
    "cup",
    "fork",
    "knife",
    "spoon",
    "bowl",
    "banana",
    "apple",
    "sandwich",
    "orange",
    "broccoli",
    "carrot",
    "hot dog",
    "pizza",
    "donut",
    "cake",
    "chair",
    "couch",
    "potted plant",
    "bed",
    "dining table",
    "toilet",
    "tv",
    "laptop",
    "mouse",
    "remote",
    "keyboard",
    "cell phone",
    "microwave",
    "oven",
    "toaster",
    "sink",
    "refrigerator",
    "book",
    "clock",
    "vase",
    "scissors",
    "teddy bear",
    "hair drier",
    "toothbrush",
];

/// COCO category ids (1–90, with gaps) of the 80 classes, in order.
const COCO_IDS: [usize; 80] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 27, 28,
    31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55,
    56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 67, 70, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 84,
    85, 86, 87, 88, 89, 90,
];

fn rgb(jpeg: &[u8]) -> Result<(Vec<u8>, usize, usize), String> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(jpeg), options);
    let pixels = decoder.decode().map_err(|e| format!("JPEG: {e:?}"))?;
    let (w, h) = decoder.dimensions().ok_or("JPEG without dimensions")?;
    Ok((pixels, w, h))
}

/// The picture stretched to `w`×`h` (bilinear), RGB planes, values 0–1.
fn stretch(px: &[u8], width: usize, height: usize, w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0f32; 3 * w * h];
    let (rx, ry) = (width as f32 / w as f32, height as f32 / h as f32);
    let at = |x: usize, y: usize, c: usize| f32::from(px[(y * width + x) * 3 + c]) / 255.0;
    for y in 0..h {
        let sy = ((y as f32 + 0.5) * ry - 0.5).clamp(0.0, (height - 1) as f32);
        let (y0, fy) = (sy.floor() as usize, sy.fract());
        let y1 = (y0 + 1).min(height - 1);
        for x in 0..w {
            let sx = ((x as f32 + 0.5) * rx - 0.5).clamp(0.0, (width - 1) as f32);
            let (x0, fx) = (sx.floor() as usize, sx.fract());
            let x1 = (x0 + 1).min(width - 1);
            for c in 0..3 {
                let top = at(x0, y0, c) * (1.0 - fx) + at(x1, y0, c) * fx;
                let bottom = at(x0, y1, c) * (1.0 - fx) + at(x1, y1, c) * fx;
                out[c * w * h + y * w + x] = top * (1.0 - fy) + bottom * fy;
            }
        }
    }
    out
}

/// Found object: label, score, box as fractions of the picture.
type Found = (String, f32, [f32; 4]);

struct Model {
    kind: String,
    plan: std::sync::Arc<TypedRunnableModel>,
    w: usize,
    h: usize,
}

impl Model {
    fn load(kind: &str, path: &str) -> Result<Self, String> {
        let mut model = tract_onnx::onnx()
            .model_for_path(path)
            .map_err(|e| format!("cannot read: {e}"))?;
        let shape = model
            .input_fact(0)
            .ok()
            .and_then(|f| f.shape.as_concrete_finite().ok().flatten())
            .map(|s| s.to_vec())
            .unwrap_or_default();
        let (h, w) = match shape.as_slice() {
            [1, 3, h, w] => (*h, *w),
            _ => (640, 640),
        };
        model = model
            .with_input_fact(0, f32::fact([1, 3, h, w]).into())
            .map_err(|e| e.to_string())?;
        let plan = model
            .into_optimized()
            .and_then(|m| m.into_runnable())
            .map_err(|e| format!("not supported by tract: {e:?}"))?;
        Ok(Self {
            kind: kind.to_owned(),
            plan,
            w,
            h,
        })
    }

    fn run(&self, px: &[u8], width: usize, height: usize) -> Result<Vec<Found>, String> {
        let mut input = stretch(px, width, height, self.w, self.h);
        if self.kind == "rfdetr" {
            let plane = self.w * self.h;
            for (c, (mean, std)) in [(0.485, 0.229), (0.456, 0.224), (0.406, 0.225)]
                .into_iter()
                .enumerate()
            {
                for v in &mut input[c * plane..(c + 1) * plane] {
                    *v = (*v - mean) / std;
                }
            }
        }
        let tensor: Tensor = tract_ndarray::Array4::from_shape_vec((1, 3, self.h, self.w), input)
            .map_err(|e| e.to_string())?
            .into();
        let out = self
            .plan
            .run(tvec!(tensor.into()))
            .map_err(|e| format!("inference: {e:?}"))?;
        let mut found = Vec::new();
        if self.kind == "yolov9" {
            let rows = out[0]
                .to_plain_array_view::<f32>()
                .map_err(|e| e.to_string())?;
            let flat: Vec<f32> = rows.iter().copied().collect();
            for r in flat.as_chunks::<6>().0 {
                let class = r[5] as usize;
                found.push((
                    COCO.get(class).unwrap_or(&"?").to_string(),
                    r[4],
                    [
                        r[0] / self.w as f32,
                        r[1] / self.h as f32,
                        r[2] / self.w as f32,
                        r[3] / self.h as f32,
                    ],
                ));
            }
        } else {
            let dets = out[0]
                .to_plain_array_view::<f32>()
                .map_err(|e| e.to_string())?;
            let logits = out[1]
                .to_plain_array_view::<f32>()
                .map_err(|e| e.to_string())?;
            let dets: Vec<f32> = dets.iter().copied().collect();
            let logits: Vec<f32> = logits.iter().copied().collect();
            let classes = logits.len() / (dets.len() / 4);
            for (q, b) in dets.as_chunks::<4>().0.iter().enumerate() {
                let row = &logits[q * classes..(q + 1) * classes];
                let (id, best) =
                    row.iter().enumerate().fold(
                        (0, f32::MIN),
                        |a, (i, s)| if *s > a.1 { (i, *s) } else { a },
                    );
                let score = 1.0 / (1.0 + (-best).exp());
                if score < 0.3 {
                    continue;
                }
                let label = COCO_IDS
                    .iter()
                    .position(|c| *c == id)
                    .map_or("?", |i| COCO[i]);
                found.push((
                    label.to_owned(),
                    score,
                    [
                        b[0] - b[2] / 2.0,
                        b[1] - b[3] / 2.0,
                        b[0] + b[2] / 2.0,
                        b[1] + b[3] / 2.0,
                    ],
                ));
            }
        }
        Ok(found)
    }
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: model_trial yolox|yolov9|rfdetr MODEL.onnx PICTURE.jpg…";
    let kind = args.next().ok_or(usage)?;
    let path = args.next().ok_or(usage)?;
    let started = Instant::now();
    let yolox;
    let other;
    if kind == "yolox" {
        yolox = Some(rustnvr_ai::onnx::OnnxDetector::load(path.as_ref())?);
        other = None;
    } else {
        yolox = None;
        other = Some(Model::load(&kind, &path)?);
    }
    println!(
        "{kind} {path}: loaded in {} ms",
        started.elapsed().as_millis()
    );
    let mut times = Vec::new();
    for picture in args {
        let jpeg = std::fs::read(&picture).map_err(|e| format!("{picture}: {e}"))?;
        let started = Instant::now();
        let found: Vec<Found> = if let Some(d) = &yolox {
            d.detect_blocking(&jpeg)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|o| (o.label, o.score, o.bbox.unwrap_or_default()))
                .collect()
        } else if let Some(m) = &other {
            let (px, w, h) = rgb(&jpeg)?;
            m.run(&px, w, h)?
        } else {
            Vec::new()
        };
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        times.push(ms);
        let shown: Vec<String> = found
            .iter()
            .filter(|f| f.1 >= 0.3)
            .map(|(l, s, b)| {
                format!(
                    "{l} {s:.2} [{:.2},{:.2},{:.2},{:.2}]",
                    b[0], b[1], b[2], b[3]
                )
            })
            .collect();
        let name = picture.rsplit(['/', '\\']).next().unwrap_or(&picture);
        println!("  {name} {ms:.0} ms: {}", shown.join(", "));
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
