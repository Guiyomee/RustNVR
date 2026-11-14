//! Motion detection on small grey pictures: difference with a slowly
//! adapting background, counted inside zones.
//!
//! Pure computation, independent of video decoding, so that it can be tested
//! with synthetic pictures. A 320×180 picture costs well under a millisecond.

use rustnvr_core::MotionSettings;

/// Background adaptation time constant: a change that stays (a parked car,
/// a shadow) stops counting as motion after a few times this duration.
const BACKGROUND_SECONDS: f32 = 4.0;
/// Time constant for pixels that did not change since the previous picture:
/// what stays still (the background uncovered by an object that left, a
/// parked car) joins the background quickly, which shortens the "ghost" an
/// object leaves behind; moving objects keep changing and are not absorbed.
const STILL_SECONDS: f32 = 1.0;
/// Largest grey level change between two pictures for a pixel to be still.
const STILL_PIXEL_THRESHOLD: u8 = 6;
/// Above this fraction of changed pixels in the whole picture, the change is
/// taken for a lighting change (day/night switch, auto exposure, a cloud)
/// rather than motion: the background is reset.
const LIGHTING_CHANGE_RATIO: f32 = 0.6;
/// Pixel difference used for the lighting change test.
const LIGHTING_PIXEL_THRESHOLD: u8 = 25;

/// Grey level difference above which a pixel changed, for a sensitivity of 1..100.
fn pixel_threshold(sensitivity: u8) -> u8 {
    let s = u32::from(sensitivity.clamp(1, 100));
    // 1 → 40, 50 → 25, 100 → 10.
    (10 + (100 - s) * 30 / 99) as u8
}

/// Fraction of a zone that must change, for a sensitivity of 1..100.
fn min_changed_ratio(sensitivity: u8) -> f32 {
    let s = f32::from(sensitivity.clamp(1, 100));
    // 1 → 2 %, 50 → about 1 %, 100 → 0.1 %.
    0.001 + (100.0 - s) / 99.0 * 0.019
}

struct Zone {
    /// Indices of the pixels inside the zone.
    pixels: Vec<u32>,
    threshold: u8,
    min_ratio: f32,
}

/// Result of analysing one picture.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    /// Motion in at least one zone.
    pub motion: bool,
    /// Largest changed fraction among zones, 0..1.
    pub score: f32,
    /// Indices of the zones with motion (into the settings' zones; empty
    /// without zones).
    pub zones: Vec<usize>,
    /// Centre of the changed pixels (picture fractions), with motion.
    pub centre: Option<[f32; 2]>,
}

/// Motion detector for one camera and one picture size.
pub struct Detector {
    width: usize,
    height: usize,
    alpha: f32,
    alpha_still: f32,
    /// Previous blurred picture.
    previous: Vec<u8>,
    background: Vec<f32>,
    blurred: Vec<u8>,
    zones: Vec<Zone>,
    /// Whether `zones` come from the settings (else: the whole picture).
    has_zones: bool,
    /// Pixels not hidden by a mask, for the centre of motion.
    visible: Vec<u32>,
    /// Change threshold for the centre of motion.
    centre_threshold: u8,
}

/// Even-odd point-in-polygon test.
fn inside(x: f32, y: f32, poly: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let ([xi, yi], [xj, yj]) = (poly[i], poly[j]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

impl Detector {
    /// Detector for `width`×`height` grey pictures analysed `fps` times per second.
    pub fn new(width: usize, height: usize, fps: u32, settings: &MotionSettings) -> Self {
        let mut zones: Vec<Zone> = settings
            .zones
            .iter()
            .filter(|z| z.points.len() >= 3)
            .map(|z| {
                let mut pixels = Vec::new();
                for y in 0..height {
                    for x in 0..width {
                        // Pixel centres, in picture fractions.
                        let (fx, fy) = (
                            (x as f32 + 0.5) / width as f32,
                            (y as f32 + 0.5) / height as f32,
                        );
                        if inside(fx, fy, &z.points) && !settings.masked(fx, fy) {
                            pixels.push((y * width + x) as u32);
                        }
                    }
                }
                Zone {
                    pixels,
                    threshold: pixel_threshold(z.sensitivity),
                    min_ratio: min_changed_ratio(z.sensitivity),
                }
            })
            .collect();
        // The whole picture, except what the privacy masks hide.
        let visible: Vec<u32> = (0..(width * height) as u32)
            .filter(|&p| {
                let (x, y) = (p as usize % width, p as usize / width);
                !settings.masked(
                    (x as f32 + 0.5) / width as f32,
                    (y as f32 + 0.5) / height as f32,
                )
            })
            .collect();
        let has_zones = !zones.is_empty();
        if !has_zones {
            zones.push(Zone {
                pixels: visible.clone(),
                threshold: pixel_threshold(settings.sensitivity),
                min_ratio: min_changed_ratio(settings.sensitivity),
            });
        }
        Self {
            width,
            height,
            alpha: 1.0 / (fps.max(1) as f32 * BACKGROUND_SECONDS),
            alpha_still: (1.0 / (fps.max(1) as f32 * STILL_SECONDS)).min(1.0),
            previous: Vec::new(),
            background: Vec::new(),
            blurred: vec![0; width * height],
            zones,
            has_zones,
            visible,
            centre_threshold: pixel_threshold(settings.sensitivity),
        }
    }

    /// 3×3 box blur, to remove sensor and compression noise.
    fn blur(&mut self, picture: &[u8]) {
        let (w, h) = (self.width, self.height);
        for y in 0..h {
            let (y0, y1) = (y.saturating_sub(1), (y + 1).min(h - 1));
            for x in 0..w {
                let (x0, x1) = (x.saturating_sub(1), (x + 1).min(w - 1));
                let mut sum = 0u32;
                let mut n = 0u32;
                for yy in y0..=y1 {
                    for xx in x0..=x1 {
                        sum += u32::from(picture[yy * w + xx]);
                        n += 1;
                    }
                }
                self.blurred[y * w + x] = (sum / n) as u8;
            }
        }
    }

    /// Analyses the next picture (`width`×`height` bytes, one per pixel).
    pub fn process(&mut self, picture: &[u8]) -> Analysis {
        if picture.len() != self.width * self.height {
            return Analysis::default();
        }
        self.blur(picture);
        if self.background.is_empty() {
            self.background = self.blurred.iter().map(|&v| f32::from(v)).collect();
            self.previous = self.blurred.clone();
            return Analysis::default();
        }
        let diff = |i: usize, bg: &[f32], cur: &[u8]| (f32::from(cur[i]) - bg[i]).abs();

        let lighting = self
            .blurred
            .iter()
            .enumerate()
            .filter(|&(i, _)| {
                diff(i, &self.background, &self.blurred) > f32::from(LIGHTING_PIXEL_THRESHOLD)
            })
            .count() as f32
            / self.blurred.len() as f32;
        if lighting > LIGHTING_CHANGE_RATIO {
            for (b, &c) in self.background.iter_mut().zip(&self.blurred) {
                *b = f32::from(c);
            }
            self.previous.copy_from_slice(&self.blurred);
            return Analysis::default();
        }

        let mut out = Analysis::default();
        for (index, zone) in self.zones.iter().enumerate() {
            if zone.pixels.is_empty() {
                continue;
            }
            let t = f32::from(zone.threshold);
            let changed = zone
                .pixels
                .iter()
                .filter(|&&p| diff(p as usize, &self.background, &self.blurred) > t)
                .count();
            let ratio = changed as f32 / zone.pixels.len() as f32;
            out.score = out.score.max(ratio);
            if ratio >= zone.min_ratio {
                out.motion = true;
                if self.has_zones {
                    out.zones.push(index);
                }
            }
        }
        if out.motion {
            // Centre of what changed, for the crossing lines.
            let t = f32::from(self.centre_threshold);
            let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0u32);
            for &p in &self.visible {
                if diff(p as usize, &self.background, &self.blurred) > t {
                    let p = p as usize;
                    sx += (p % self.width) as f64 + 0.5;
                    sy += (p / self.width) as f64 + 0.5;
                    n += 1;
                }
            }
            if n > 0 {
                out.centre = Some([
                    (sx / f64::from(n) / self.width as f64) as f32,
                    (sy / f64::from(n) / self.height as f64) as f32,
                ]);
            }
        }
        let (a, still) = (self.alpha, self.alpha_still);
        for ((b, &c), &p) in self
            .background
            .iter_mut()
            .zip(&self.blurred)
            .zip(&self.previous)
        {
            let rate = if c.abs_diff(p) <= STILL_PIXEL_THRESHOLD {
                still
            } else {
                a
            };
            *b += rate * (f32::from(c) - *b);
        }
        self.previous.copy_from_slice(&self.blurred);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnvr_core::MotionZone;

    const W: usize = 160;
    const H: usize = 90;

    /// Textured background with a bright square at `(x, y)` if any.
    fn picture(square: Option<(usize, usize)>, offset: u8) -> Vec<u8> {
        let mut p: Vec<u8> = (0..W * H)
            .map(|i| 60 + ((i * 7 + i / W * 13) % 20) as u8 + offset)
            .collect();
        if let Some((sx, sy)) = square {
            for y in sy..sy + 20 {
                for x in sx..sx + 20 {
                    p[y * W + x] = 220;
                }
            }
        }
        p
    }

    fn settings(zones: Vec<MotionZone>) -> MotionSettings {
        MotionSettings {
            enabled: true,
            sensitivity: 50,
            zones,
            objects: Vec::new(),
            masks: Vec::new(),
            lines: Vec::new(),
            schedule: Vec::new(),
        }
    }

    #[test]
    fn detects_a_moving_object_and_ignores_still_pictures() {
        let mut d = Detector::new(W, H, 5, &settings(Vec::new()));
        for _ in 0..5 {
            assert!(!d.process(&picture(None, 0)).motion);
        }
        let a = d.process(&picture(Some((10, 10)), 0));
        assert!(a.motion && a.score > 0.02, "{a:?}");
        assert!(a.zones.is_empty());
        // Noise-level changes are not motion.
        assert!(!d.process(&picture(None, 2)).motion);
    }

    #[test]
    fn lighting_changes_reset_the_background() {
        let mut d = Detector::new(W, H, 5, &settings(Vec::new()));
        d.process(&picture(None, 0));
        assert!(!d.process(&picture(None, 100)).motion, "global change");
        assert!(!d.process(&picture(None, 100)).motion, "new background");
    }

    #[test]
    fn zones_limit_where_motion_counts() {
        // Right half only.
        let right = MotionZone {
            name: "droite".into(),
            points: vec![[0.5, 0.0], [1.0, 0.0], [1.0, 1.0], [0.5, 1.0]],
            sensitivity: 50,
            objects: Vec::new(),
        };
        let mut d = Detector::new(W, H, 5, &settings(vec![right]));
        d.process(&picture(None, 0));
        assert!(!d.process(&picture(Some((10, 10)), 0)).motion, "left half");
        let a = d.process(&picture(Some((120, 40)), 0));
        assert!(a.motion);
        assert_eq!(a.zones, vec![0]);
    }

    #[test]
    fn privacy_masks_are_ignored() {
        // The left half is hidden: motion there does not count.
        let masked = MotionSettings {
            masks: vec![rustnvr_core::PrivacyMask {
                name: "voisin".into(),
                rect: [0.0, 0.0, 0.5, 1.0],
            }],
            ..settings(Vec::new())
        };
        let mut d = Detector::new(W, H, 5, &masked);
        d.process(&picture(None, 0));
        assert!(
            !d.process(&picture(Some((10, 10)), 0)).motion,
            "masked half"
        );
        assert!(d.process(&picture(Some((120, 40)), 0)).motion);
    }

    #[test]
    fn ghost_of_an_object_that_left_fades_quickly() {
        let mut d = Detector::new(W, H, 5, &settings(Vec::new()));
        for _ in 0..5 {
            d.process(&picture(None, 0));
        }
        // An object stays 5 s (absorbed into the background), then leaves.
        for _ in 0..25 {
            d.process(&picture(Some((60, 30)), 0));
        }
        let ghost = (0..25)
            .take_while(|_| d.process(&picture(None, 0)).motion)
            .count();
        // About 1.6 s at 5 pictures/s (7 s with the slow background alone).
        assert!(ghost <= 9, "ghost lasted {ghost} pictures");
        // A moving object is still detected throughout.
        let moving = (0..10)
            .filter(|i| d.process(&picture(Some((10 + i * 12, 30)), 0)).motion)
            .count();
        assert!(moving >= 9, "{moving}");
    }
    #[test]
    fn sensitivity_maps_to_thresholds() {
        assert_eq!(pixel_threshold(1), 40);
        assert_eq!(pixel_threshold(100), 10);
        assert!(min_changed_ratio(100) < min_changed_ratio(50));
        assert!((min_changed_ratio(1) - 0.02).abs() < 1e-6);
    }
}
