//! Built-in object recognition: a YOLOX model (ONNX, e.g. `yolox_nano.onnx`,
//! Apache-2.0, from the YOLOX releases) run in process by tract, in pure
//! Rust. No model is shipped: the user downloads one.
//!
//! YOLOX pre/post-processing (as in its reference demo): the picture is
//! scaled to fit the square input (aspect kept, padded with grey 114 at the
//! right/bottom), fed as BGR values 0–255 in NCHW layout; each output row is
//! `[x, y, log w, log h, objectness, 80 class scores]` on the 8/16/32 pixel
//! grids, scores already through a sigmoid.

use crate::{AiError, BoxFuture, ObjectDetector};
use rustnvr_core::DetectedObject;
use std::path::Path;
use std::sync::Arc;
use tract_onnx::prelude::*;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// Labels of the 80 COCO classes, in model order.
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

/// Candidates below this score are dropped before overlap removal (the
/// configured `min_score` filters afterwards).
const CANDIDATE_SCORE: f32 = 0.2;
/// Boxes of the same class overlapping more than this are merged.
const NMS_IOU: f32 = 0.45;
/// Grey of the padding, as in YOLOX.
const PAD: f32 = 114.0;

type Plan = Arc<TypedRunnableModel>;

/// A YOLOX ONNX model ready to run.
pub struct OnnxDetector {
    plan: Plan,
    /// Side of the square input, in pixels.
    size: usize,
}

impl OnnxDetector {
    /// Loads and optimizes the model at `path` (a few hundred ms).
    pub fn load(path: &Path) -> Result<Self, String> {
        let mut model = tract_onnx::onnx()
            .model_for_path(path)
            .map_err(|e| format!("cannot read the model {}: {e}", path.display()))?;
        // Models exported with a fixed input size declare it; others get 416.
        let declared = model
            .input_fact(0)
            .ok()
            .and_then(|f| f.shape.as_concrete_finite().ok().flatten())
            .map(|s| s.to_vec());
        let size = match declared.as_deref() {
            Some([1, 3, h, w]) if h == w => *h,
            _ => 416,
        };
        model = model
            .with_input_fact(0, f32::fact([1, 3, size, size]).into())
            .map_err(|e| e.to_string())?;
        let plan = model
            .into_optimized()
            .and_then(|m| m.into_runnable())
            .map_err(|e| format!("unsupported model {}: {e}", path.display()))?;
        Ok(Self { plan, size })
    }

    /// Input side of the model.
    pub fn input_size(&self) -> usize {
        self.size
    }

    /// Objects in `jpeg`, with boxes as fractions of the picture. Blocking.
    pub fn detect_blocking(&self, jpeg: &[u8]) -> Result<Vec<DetectedObject>, AiError> {
        let (rgb, width, height) = decode_rgb(jpeg)?;
        let (input, ratio) = letterbox(&rgb, width, height, self.size);
        let tensor: Tensor =
            tract_ndarray::Array4::from_shape_vec((1, 3, self.size, self.size), input)
                .map_err(|e| AiError::Provider(e.to_string()))?
                .into();
        let outputs = self
            .plan
            .run(tvec!(tensor.into()))
            .map_err(|e| AiError::Provider(format!("inference: {e}")))?;
        let out = outputs[0]
            .to_plain_array_view::<f32>()
            .map_err(|e| AiError::Provider(e.to_string()))?;
        let rows: Vec<&[f32]> = out
            .as_slice()
            .ok_or_else(|| AiError::Provider("unexpected output layout".to_owned()))?
            .as_chunks::<85>()
            .0
            .iter()
            .map(|r| r.as_slice())
            .collect();
        let candidates = decode(&rows, self.size, ratio);
        Ok(nms(candidates)
            .into_iter()
            .map(|c| DetectedObject {
                label: COCO[c.class].to_owned(),
                score: c.score,
                bbox: Some([
                    (c.x0 / width as f32).clamp(0.0, 1.0),
                    (c.y0 / height as f32).clamp(0.0, 1.0),
                    (c.x1 / width as f32).clamp(0.0, 1.0),
                    (c.y1 / height as f32).clamp(0.0, 1.0),
                ]),
                zone: None,
            })
            .collect())
    }
}

/// Pictures waiting for the model. Each event waits for its answer before
/// sending its next picture, so the queue stays below the number of events
/// in progress; this only bounds a model far too slow for the cameras.
const QUEUE: usize = 16;

type Job = (
    Vec<u8>,
    tokio::sync::oneshot::Sender<Result<Vec<DetectedObject>, AiError>>,
);

/// Runs every analysis on one dedicated thread, one at a time.
///
/// The allocator keeps a memory arena per thread at the size of the largest
/// analysis it ran: analyses spread over the blocking pool kept one per
/// simultaneous analysis (YOLOX-l: 410 MiB loaded, 622 MiB after three at a
/// time, `examples/onnx_memory.rs`). One thread keeps one. Pictures of
/// cameras moving together wait for each other instead.
pub struct SharedOnnx {
    jobs: std::sync::mpsc::SyncSender<Job>,
}

impl SharedOnnx {
    /// Starts the analysis thread for `detector`; it stops with the last
    /// [`SharedOnnx`].
    pub fn new(detector: OnnxDetector) -> Result<Self, String> {
        let (jobs, queue) = std::sync::mpsc::sync_channel::<Job>(QUEUE);
        std::thread::Builder::new()
            .name("ai-onnx".to_owned())
            .spawn(move || {
                for (jpeg, reply) in queue {
                    let _ = reply.send(detector.detect_blocking(&jpeg));
                }
            })
            .map_err(|e| format!("cannot start the analysis thread: {e}"))?;
        Ok(Self { jobs })
    }
}

impl ObjectDetector for SharedOnnx {
    fn name(&self) -> &str {
        "onnx"
    }

    fn detect<'a>(&'a self, jpeg: &'a [u8]) -> BoxFuture<'a, Result<Vec<DetectedObject>, AiError>> {
        let (reply, answer) = tokio::sync::oneshot::channel();
        let queued = self.jobs.try_send((jpeg.to_vec(), reply)).map_err(|e| match e {
            std::sync::mpsc::TrySendError::Full(_) => AiError::Provider(format!(
                "{QUEUE} pictures already waiting: the model is too slow for the cameras' motion, picture skipped (a lighter model keeps up)"
            )),
            std::sync::mpsc::TrySendError::Disconnected(_) => {
                AiError::Provider("analysis thread stopped".to_owned())
            }
        });
        Box::pin(async move {
            queued?;
            answer
                .await
                .map_err(|_| AiError::Provider("analysis thread stopped".to_owned()))?
        })
    }

    /// tract has no GPU support: always the processor, one core per picture.
    fn device(&self) -> Option<String> {
        Some("CPU".to_owned())
    }
}

fn decode_rgb(jpeg: &[u8]) -> Result<(Vec<u8>, usize, usize), AiError> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(jpeg), options);
    let pixels = decoder
        .decode()
        .map_err(|e| AiError::Provider(format!("JPEG: {e:?}")))?;
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| AiError::Provider("JPEG without dimensions".to_owned()))?;
    if pixels.len() != width * height * 3 {
        return Err(AiError::Provider("unexpected JPEG layout".to_owned()));
    }
    Ok((pixels, width, height))
}

/// The picture scaled into a `size`×`size` BGR planar input (bilinear,
/// aspect kept, padded right/bottom); returns it and the scale applied.
fn letterbox(rgb: &[u8], width: usize, height: usize, size: usize) -> (Vec<f32>, f32) {
    let ratio = (size as f32 / width as f32).min(size as f32 / height as f32);
    let (w, h) = (
        ((width as f32 * ratio).round() as usize).clamp(1, size),
        ((height as f32 * ratio).round() as usize).clamp(1, size),
    );
    let plane = size * size;
    let mut out = vec![PAD; 3 * plane];
    let sample = |x: usize, y: usize, c: usize| f32::from(rgb[(y * width + x) * 3 + c]);
    for y in 0..h {
        let sy = ((y as f32 + 0.5) / ratio - 0.5).clamp(0.0, (height - 1) as f32);
        let (y0, fy) = (sy.floor() as usize, sy.fract());
        let y1 = (y0 + 1).min(height - 1);
        for x in 0..w {
            let sx = ((x as f32 + 0.5) / ratio - 0.5).clamp(0.0, (width - 1) as f32);
            let (x0, fx) = (sx.floor() as usize, sx.fract());
            let x1 = (x0 + 1).min(width - 1);
            for (plane_index, c) in [2, 1, 0].into_iter().enumerate() {
                let top = sample(x0, y0, c) * (1.0 - fx) + sample(x1, y0, c) * fx;
                let bottom = sample(x0, y1, c) * (1.0 - fx) + sample(x1, y1, c) * fx;
                out[plane_index * plane + y * size + x] = top * (1.0 - fy) + bottom * fy;
            }
        }
    }
    (out, ratio)
}

#[derive(Clone, Copy, Debug)]
struct Candidate {
    class: usize,
    score: f32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

/// Output rows decoded on the 8/16/32 grids, in picture pixels.
fn decode(rows: &[&[f32]], size: usize, ratio: f32) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut index = 0;
    for stride in [8usize, 16, 32] {
        let cells = size / stride;
        for gy in 0..cells {
            for gx in 0..cells {
                let Some(r) = rows.get(index) else {
                    return out;
                };
                index += 1;
                let (class, best) =
                    r[5..85]
                        .iter()
                        .copied()
                        .enumerate()
                        .fold(
                            (0, f32::MIN),
                            |acc, (i, s)| if s > acc.1 { (i, s) } else { acc },
                        );
                let score = r[4] * best;
                if score < CANDIDATE_SCORE {
                    continue;
                }
                let s = stride as f32;
                let cx = (r[0] + gx as f32) * s / ratio;
                let cy = (r[1] + gy as f32) * s / ratio;
                let w = r[2].exp() * s / ratio;
                let h = r[3].exp() * s / ratio;
                out.push(Candidate {
                    class,
                    score,
                    x0: cx - w / 2.0,
                    y0: cy - h / 2.0,
                    x1: cx + w / 2.0,
                    y1: cy + h / 2.0,
                });
            }
        }
    }
    out
}

fn iou(a: &Candidate, b: &Candidate) -> f32 {
    let w = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
    let h = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
    let inter = w * h;
    let area = |c: &Candidate| (c.x1 - c.x0).max(0.0) * (c.y1 - c.y0).max(0.0);
    inter / (area(a) + area(b) - inter).max(f32::EPSILON)
}

/// Keeps the best box among those of the same class that overlap.
fn nms(mut candidates: Vec<Candidate>) -> Vec<Candidate> {
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Candidate> = Vec::new();
    for c in candidates {
        if kept
            .iter()
            .all(|k| k.class != c.class || iou(k, &c) <= NMS_IOU)
        {
            kept.push(c);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterbox_keeps_aspect_and_pads() {
        // 4×2 red picture into an 8×8 input: scaled ×2 to 8×4, padded below.
        let rgb: Vec<u8> = [255u8, 0, 0].repeat(8);
        let (input, ratio) = letterbox(&rgb, 4, 2, 8);
        assert!((ratio - 2.0).abs() < f32::EPSILON);
        let plane = 64;
        // BGR planes: blue 0, green 0, red 255 in the picture area.
        assert_eq!(input[0], 0.0);
        assert_eq!(input[plane], 0.0);
        assert_eq!(input[2 * plane], 255.0);
        // Below the picture: padding.
        assert_eq!(input[2 * plane + 5 * 8], PAD);
    }

    #[test]
    fn decodes_grids_and_merges_overlaps() {
        // One strong "person" in the first 8-pixel cell, a weaker duplicate
        // next to it, and a weak "car" dropped by the candidate threshold.
        let size = 32;
        let cells = (size / 8) * (size / 8) + (size / 16) * (size / 16) + 1;
        let mut data = vec![vec![0f32; 85]; cells];
        let set = |row: &mut Vec<f32>, obj: f32, class: usize, score: f32| {
            row[2] = (2.0f32).ln(); // 16 px wide
            row[3] = (2.0f32).ln();
            row[4] = obj;
            row[5 + class] = score;
        };
        set(&mut data[0], 0.9, 0, 0.9);
        set(&mut data[1], 0.8, 0, 0.6);
        data[1][0] = -1.0; // same centre as the first box: a duplicate
        set(&mut data[2], 0.3, 2, 0.3);
        let rows: Vec<&[f32]> = data.iter().map(Vec::as_slice).collect();
        let found = nms(decode(&rows, size, 1.0));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].class, 0);
        assert!((found[0].score - 0.81).abs() < 1e-5);
        // Centre of cell (0, 0) on the 8-pixel grid, 16 px box.
        assert!((found[0].x0 + 8.0).abs() < 1e-4 && (found[0].x1 - 8.0).abs() < 1e-4);
    }
}
