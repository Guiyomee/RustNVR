//! License plate reading on the vehicles recognized: two small ONNX models
//! downloaded by the user (`crate::models::PLATE_MODELS`), run in process by
//! ONNX Runtime when its library is installed, else by tract.
//!
//! 1. The plate detector (YOLOv9-t of open-image-models, MIT) gets the
//!    vehicle, cut from the picture with a margin: scaled into its square
//!    input (aspect kept, centred on grey 114), RGB values 0–1 in NCHW
//!    layout. Its rows are `[batch, x1, y1, x2, y2, class, score]` in input
//!    pixels, overlaps already removed inside the model.
//! 2. The reader (CCT-S v2 of fast-plate-ocr, MIT) gets the plate stretched
//!    to 128×64, RGB bytes in NHWC layout, and gives, for each of its 10
//!    slots, the probabilities of the 37 symbols (`_` pads short plates).
//!
//! A plate is kept when it is whole (not cut by the picture's edge), wide
//! enough and every symbol read is sure enough: on 89 photos of French
//! plates, the misreadings left at 0.9 were plates cut or hidden in part, or
//! narrower than 50 pixels.

use crate::AiError;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tract_onnx::prelude::*;

/// Symbols of the reader, in output order.
const ALPHABET: &[u8; 37] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ_";
/// Padding symbol of the reader.
const PAD_SYMBOL: u8 = b'_';
/// Slots of the reader (longest plate).
const SLOTS: usize = 10;
/// Input of the reader.
const READER_WIDTH: usize = 128;
const READER_HEIGHT: usize = 64;
/// Plates found below this score are ignored.
const DETECTOR_SCORE: f32 = 0.4;
/// Least probability of each symbol of a plate kept.
pub const READ_CONFIDENCE: f32 = 0.95;
/// Shortest plate kept (a cut plate gives 2 to 4 symbols).
const MIN_SYMBOLS: usize = 5;
/// Narrowest plate kept, in pixels of the picture.
const MIN_PLATE_PIXELS: f32 = 60.0;
/// A plate this close to the picture's edge (pixels) is cut.
const EDGE: f32 = 2.0;
/// Margin added around the vehicle box, as a share of its size: the
/// vehicle may have moved a little since the picture it was found on.
const VEHICLE_MARGIN: f32 = 0.15;
/// Vehicles narrower than this in the picture have unreadable plates.
const MIN_VEHICLE_PIXELS: usize = 96;
/// Grey of the detector's padding.
const PAD_GREY: u8 = 114;

/// A plate read.
#[derive(Clone, Debug, PartialEq)]
pub struct Plate {
    /// Letters and digits, without separators (`AB123CD`).
    pub text: String,
    /// Least probability of its symbols.
    pub confidence: f32,
    /// Width of the plate in the picture, in pixels.
    pub width: u32,
}

/// What runs one model.
enum Engine {
    Ort(Mutex<ort::session::Session>),
    Tract(Arc<TypedRunnableModel>),
}

impl Engine {
    fn name(&self) -> &'static str {
        match self {
            Engine::Ort(_) => "ONNX Runtime",
            Engine::Tract(_) => "tract",
        }
    }
}

/// The two plate models, ready to run.
pub struct PlateReader {
    detector: Engine,
    /// Side of the detector's square input.
    size: usize,
    reader: Engine,
}

impl std::fmt::Debug for PlateReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlateReader")
            .field("engine", &self.engine())
            .field("size", &self.size)
            .finish()
    }
}

impl PlateReader {
    /// Loads the detector and the reader: with ONNX Runtime when its
    /// library is installed, else (or when it fails) with tract.
    pub fn load(detector: &Path, reader: &Path) -> Result<Self, String> {
        match (
            crate::ort_engine::open(detector),
            crate::ort_engine::open(reader),
        ) {
            (Ok(Some((d, ..))), Ok(Some((r, ..)))) => {
                let size = crate::ort_engine::square_input(&d).unwrap_or(384);
                return Ok(Self {
                    detector: Engine::Ort(Mutex::new(d)),
                    size,
                    reader: Engine::Ort(Mutex::new(r)),
                });
            }
            (Ok(None), _) | (_, Ok(None)) => {}
            (Err(e), _) | (_, Err(e)) => {
                tracing::warn!(error = %e, "ONNX Runtime not usable: plate reading runs on tract");
            }
        }
        Self::load_tract(detector, reader)
    }

    /// Both models with tract.
    pub fn load_tract(detector: &Path, reader: &Path) -> Result<Self, String> {
        let open = |path: &Path| {
            tract_onnx::onnx()
                .model_for_path(path)
                .map_err(|e| format!("cannot read the model {}: {e}", path.display()))
        };
        let mut model = open(detector)?;
        let declared = model
            .input_fact(0)
            .ok()
            .and_then(|f| f.shape.as_concrete_finite().ok().flatten())
            .map(|s| s.to_vec());
        let size = match declared.as_deref() {
            Some([1, 3, h, w]) if h == w => *h,
            _ => 384,
        };
        model = model
            .with_input_fact(0, f32::fact([1, 3, size, size]).into())
            .map_err(|e| e.to_string())?;
        let detector_plan = model
            .into_optimized()
            .and_then(|m| m.into_runnable())
            .map_err(|e| format!("unsupported model {}: {e}", detector.display()))?;
        let mut model = open(reader)?;
        model = model
            .with_input_fact(0, u8::fact([1, READER_HEIGHT, READER_WIDTH, 3]).into())
            .map_err(|e| e.to_string())?;
        // The plate only, not the country.
        model
            .select_outputs_by_name(["plate"])
            .map_err(|e| e.to_string())?;
        let reader_plan = model
            .into_optimized()
            .and_then(|m| m.into_runnable())
            .map_err(|e| format!("unsupported model {}: {e}", reader.display()))?;
        Ok(Self {
            detector: Engine::Tract(detector_plan),
            size,
            reader: Engine::Tract(reader_plan),
        })
    }

    /// The engine running the models: `ONNX Runtime` or `tract`.
    pub fn engine(&self) -> &'static str {
        self.reader.name()
    }

    /// The plate of each vehicle of `jpeg` (boxes as fractions of the
    /// picture), `None` where none is read surely. Blocking.
    pub fn read_blocking(
        &self,
        jpeg: &[u8],
        vehicles: &[[f32; 4]],
    ) -> Result<Vec<Option<Plate>>, AiError> {
        let (rgb, width, height) = crate::onnx::decode_rgb(jpeg)?;
        let picture = Picture {
            rgb: &rgb,
            width,
            height,
        };
        vehicles
            .iter()
            .map(|b| self.read_vehicle(&picture, *b))
            .collect()
    }

    fn read_vehicle(&self, picture: &Picture<'_>, b: [f32; 4]) -> Result<Option<Plate>, AiError> {
        let (w, h) = (picture.width as f32, picture.height as f32);
        let (mx, my) = (
            (b[2] - b[0]) * VEHICLE_MARGIN,
            (b[3] - b[1]) * VEHICLE_MARGIN,
        );
        let area = Rect {
            x0: ((b[0] - mx) * w).max(0.0),
            y0: ((b[1] - my) * h).max(0.0),
            x1: ((b[2] + mx) * w).min(w),
            y1: ((b[3] + my) * h).min(h),
        };
        if area.width() < MIN_VEHICLE_PIXELS as f32 || area.height() < 8.0 {
            return Ok(None);
        }
        let Some(plate) = self.find_plate(picture, area)? else {
            return Ok(None);
        };
        let (w, h) = (picture.width as f32, picture.height as f32);
        let cut = plate.x0 < EDGE || plate.y0 < EDGE || plate.x1 > w - EDGE || plate.y1 > h - EDGE;
        if cut || plate.width() < MIN_PLATE_PIXELS || plate.height() < 4.0 {
            return Ok(None);
        }
        let input = picture.resize(plate, READER_WIDTH, READER_HEIGHT);
        let probabilities = self.run_reader(input)?;
        Ok(
            decode_plate(&probabilities).map(|(text, confidence)| Plate {
                text,
                confidence,
                width: plate.width().round() as u32,
            }),
        )
    }

    /// The best plate in `area` of the picture, in picture pixels.
    fn find_plate(&self, picture: &Picture<'_>, area: Rect) -> Result<Option<Rect>, AiError> {
        let size = self.size;
        let ratio = (size as f32 / area.width()).min(size as f32 / area.height());
        let (w, h) = (
            ((area.width() * ratio).round() as usize).clamp(1, size),
            ((area.height() * ratio).round() as usize).clamp(1, size),
        );
        let scaled = picture.resize(area, w, h);
        let (dw, dh) = ((size - w) as f32 / 2.0, (size - h) as f32 / 2.0);
        let (left, top) = ((dw - 0.1).round() as usize, (dh - 0.1).round() as usize);
        let plane = size * size;
        let mut input = vec![f32::from(PAD_GREY) / 255.0; 3 * plane];
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    input[c * plane + (y + top) * size + x + left] =
                        f32::from(scaled[(y * w + x) * 3 + c]) / 255.0;
                }
            }
        }
        let rows = match &self.detector {
            Engine::Ort(session) => {
                let mut session = session.lock().unwrap_or_else(|e| e.into_inner());
                crate::ort_engine::run_input(&mut session, [1, 3, size, size], input, None)
                    .map_err(AiError::Provider)?
            }
            Engine::Tract(plan) => {
                let tensor: Tensor =
                    tract_ndarray::Array4::from_shape_vec((1, 3, size, size), input)
                        .map_err(|e| AiError::Provider(e.to_string()))?
                        .into();
                tract_values(plan, tensor)?
            }
        };
        let best = rows
            .as_chunks::<7>()
            .0
            .iter()
            .filter(|r| r[6] >= DETECTOR_SCORE)
            .max_by(|a, b| a[6].total_cmp(&b[6]));
        Ok(best.map(|r| Rect {
            x0: ((r[1] - dw) / ratio + area.x0).clamp(area.x0, area.x1),
            y0: ((r[2] - dh) / ratio + area.y0).clamp(area.y0, area.y1),
            x1: ((r[3] - dw) / ratio + area.x0).clamp(area.x0, area.x1),
            y1: ((r[4] - dh) / ratio + area.y0).clamp(area.y0, area.y1),
        }))
    }

    /// The reader's probabilities (10 slots × 37 symbols).
    fn run_reader(&self, input: Vec<u8>) -> Result<Vec<f32>, AiError> {
        match &self.reader {
            Engine::Ort(session) => {
                let mut session = session.lock().unwrap_or_else(|e| e.into_inner());
                crate::ort_engine::run_input(
                    &mut session,
                    [1, READER_HEIGHT, READER_WIDTH, 3],
                    input,
                    Some("plate"),
                )
                .map_err(AiError::Provider)
            }
            Engine::Tract(plan) => {
                let tensor: Tensor = tract_ndarray::Array4::from_shape_vec(
                    (1, READER_HEIGHT, READER_WIDTH, 3),
                    input,
                )
                .map_err(|e| AiError::Provider(e.to_string()))?
                .into();
                tract_values(plan, tensor)
            }
        }
    }
}

/// The values of the first output of `plan` run on `input`.
fn tract_values(plan: &Arc<TypedRunnableModel>, input: Tensor) -> Result<Vec<f32>, AiError> {
    let outputs = plan
        .run(tvec!(input.into()))
        .map_err(|e| AiError::Provider(format!("inference: {e}")))?;
    let view = outputs[0]
        .to_plain_array_view::<f32>()
        .map_err(|e| AiError::Provider(e.to_string()))?;
    Ok(view.iter().copied().collect())
}

/// The text read from the reader's output and the least probability of its
/// symbols, `None` when too short or not sure enough.
fn decode_plate(probabilities: &[f32]) -> Option<(String, f32)> {
    let mut text = String::new();
    let mut confidence = 1.0f32;
    let mut symbols = Vec::with_capacity(SLOTS);
    for slot in probabilities.as_chunks::<37>().0.iter().take(SLOTS) {
        let (index, p) = slot
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        symbols.push((ALPHABET[index], p));
    }
    // Padding only at the end (as fast-plate-ocr removes it).
    while symbols.last().is_some_and(|(s, _)| *s == PAD_SYMBOL) {
        symbols.pop();
    }
    for (symbol, p) in symbols {
        if symbol == PAD_SYMBOL {
            return None;
        }
        text.push(char::from(symbol));
        confidence = confidence.min(p);
    }
    (text.len() >= MIN_SYMBOLS && confidence >= READ_CONFIDENCE).then_some((text, confidence))
}

/// A rectangle of the picture, in pixels.
#[derive(Clone, Copy, Debug)]
struct Rect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Rect {
    fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    fn height(&self) -> f32 {
        self.y1 - self.y0
    }
}

/// A decoded RGB picture.
struct Picture<'a> {
    rgb: &'a [u8],
    width: usize,
    height: usize,
}

impl Picture<'_> {
    /// `area` scaled (bilinear) to `w`×`h`, RGB bytes.
    fn resize(&self, area: Rect, w: usize, h: usize) -> Vec<u8> {
        let mut out = vec![0u8; w * h * 3];
        let (sx, sy) = (area.width() / w as f32, area.height() / h as f32);
        let max_x = (self.width - 1) as f32;
        let max_y = (self.height - 1) as f32;
        for y in 0..h {
            let fy = (area.y0 + (y as f32 + 0.5) * sy - 0.5).clamp(0.0, max_y);
            let (y0, ty) = (fy.floor() as usize, fy.fract());
            let y1 = (y0 + 1).min(self.height - 1);
            for x in 0..w {
                let fx = (area.x0 + (x as f32 + 0.5) * sx - 0.5).clamp(0.0, max_x);
                let (x0, tx) = (fx.floor() as usize, fx.fract());
                let x1 = (x0 + 1).min(self.width - 1);
                for c in 0..3 {
                    let at = |x: usize, y: usize| f32::from(self.rgb[(y * self.width + x) * 3 + c]);
                    let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
                    let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
                    out[(y * w + x) * 3 + c] = (top * (1.0 - ty) + bottom * ty).round() as u8;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reader output where each slot is sure of one symbol.
    fn output(symbols: &str, p: f32) -> Vec<f32> {
        let mut out = vec![0.0; SLOTS * ALPHABET.len()];
        let padded = format!("{symbols:_<10}");
        for (slot, s) in padded.bytes().enumerate() {
            let index = ALPHABET.iter().position(|a| *a == s).expect("symbol");
            out[slot * ALPHABET.len() + index] = p;
        }
        out
    }

    #[test]
    fn decodes_sure_plates_only() {
        assert_eq!(
            decode_plate(&output("AB123CD", 0.97)),
            Some(("AB123CD".to_owned(), 0.97))
        );
        // Not sure enough, too short, blank plate.
        assert_eq!(decode_plate(&output("AB123CD", 0.6)), None);
        assert_eq!(decode_plate(&output("AB12", 0.99)), None);
        assert_eq!(decode_plate(&output("", 0.99)), None);
        // Padding in the middle: a misread.
        assert_eq!(decode_plate(&output("AB_23CD", 0.99)), None);
    }

    #[test]
    fn resizes_an_area() {
        // 4×2 picture: left half red, right half blue; its right half
        // scaled to 1×1 is blue.
        let mut rgb = Vec::new();
        for _ in 0..2 {
            rgb.extend_from_slice(&[255, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0, 255]);
        }
        let picture = Picture {
            rgb: &rgb,
            width: 4,
            height: 2,
        };
        let area = Rect {
            x0: 2.0,
            y0: 0.0,
            x1: 4.0,
            y1: 2.0,
        };
        assert_eq!(picture.resize(area, 1, 1), vec![0, 0, 255]);
    }
}
