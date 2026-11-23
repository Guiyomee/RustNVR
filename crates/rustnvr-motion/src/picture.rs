//! Checks of the picture itself while the stream flows: a picture that no
//! longer changes at all (the camera's encoder is stuck) or that became
//! uniform at once (lens covered, camera masked, light switched off).
//!
//! The « camera offline » alarm only sees a stream that stops; these see a
//! stream that goes on with a useless picture. Both look at the small grey
//! pictures of the motion detection, once a second, on one visible pixel in
//! four: a few thousand comparisons a second per camera.

use crate::detector::visible_pixels;
use rustnvr_core::{MotionSettings, PictureProblem};

/// One picture looked at per…
const EVERY_MS: i64 = 1_000;
/// A frozen picture is reported after…
pub const FROZEN_AFTER_MS: i64 = 5 * 60_000;
/// A covered picture is reported after…
pub const COVERED_AFTER_MS: i64 = 60_000;
/// Grey levels a pixel must move by to count as changed. The pictures of a
/// stuck encoder decode identical; a real picture has noise, and each key
/// frame encodes it again.
const CHANGED_LEVELS: u8 = 2;
/// A picture is the same as the reference while at most one pixel in this
/// many changed.
const SAME_RATIO: usize = 5_000;
/// Spread of the grey levels (standard deviation) below which a picture is
/// uniform…
const UNIFORM_BELOW: f32 = 5.0;
/// … and from which it shows something (again).
const DETAIL_FROM: f32 = 12.0;
/// A uniform picture counts as covered only when it showed something this
/// shortly before: night falling darkens a picture slowly, not at once.
const SUDDEN_MS: i64 = 10_000;
/// One pixel in…
const STEP: usize = 4;

/// Picture checks of one camera, for one picture size.
#[derive(Debug)]
pub struct PictureCheck {
    /// Sampled pixels (visible ones only).
    pixels: Vec<u32>,
    /// Bytes of a picture.
    len: usize,
    last_ms: Option<i64>,
    /// Sampled values of the picture the next ones are compared with.
    reference: Vec<u8>,
    /// Since when the picture is the same as `reference`.
    same_since_ms: i64,
    /// Last time the picture showed something.
    detail_ms: Option<i64>,
    /// Since when the picture is uniform, after becoming so at once.
    uniform_since_ms: Option<i64>,
    reported: Option<PictureProblem>,
}

/// Standard deviation of `values`.
fn spread(values: &[u8]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let n = values.len() as f64;
    let mean = values.iter().map(|&v| f64::from(v)).sum::<f64>() / n;
    let var = values
        .iter()
        .map(|&v| (f64::from(v) - mean).powi(2))
        .sum::<f64>()
        / n;
    var.sqrt() as f32
}

impl PictureCheck {
    /// Checks for `width`×`height` grey pictures; the privacy masks of
    /// `settings` are left out (what they hide may move or not).
    pub fn new(width: usize, height: usize, settings: &MotionSettings) -> Self {
        Self {
            pixels: visible_pixels(width, height, settings)
                .into_iter()
                .step_by(STEP)
                .collect(),
            len: width * height,
            last_ms: None,
            reference: Vec::new(),
            same_since_ms: 0,
            detail_ms: None,
            uniform_since_ms: None,
            reported: None,
        }
    }

    /// Bytes of the pictures checked.
    pub fn picture_len(&self) -> usize {
        self.len
    }

    /// What is reported now.
    pub fn reported(&self) -> Option<PictureProblem> {
        self.reported
    }

    /// Looks at `picture`, taken at `now_ms`. Returns what is reported when
    /// it changes: `Some(Some(problem))` for a new problem, `Some(None)` when
    /// the picture is right again.
    pub fn update(&mut self, now_ms: i64, picture: &[u8]) -> Option<Option<PictureProblem>> {
        if picture.len() != self.len || self.pixels.is_empty() {
            return None;
        }
        if self.last_ms.is_some_and(|t| now_ms - t < EVERY_MS) {
            return None;
        }
        self.last_ms = Some(now_ms);
        let values: Vec<u8> = self.pixels.iter().map(|&p| picture[p as usize]).collect();

        let changed = if self.reference.len() == values.len() {
            values
                .iter()
                .zip(&self.reference)
                .filter(|(a, b)| a.abs_diff(**b) > CHANGED_LEVELS)
                .count()
        } else {
            values.len()
        };
        if changed * SAME_RATIO > values.len() {
            self.reference = values.clone();
            self.same_since_ms = now_ms;
        }

        let spread = spread(&values);
        let had_detail = self.detail_ms.is_some_and(|t| now_ms - t <= SUDDEN_MS);
        if spread >= DETAIL_FROM {
            self.detail_ms = Some(now_ms);
            self.uniform_since_ms = None;
        } else if spread < UNIFORM_BELOW {
            if self.uniform_since_ms.is_none() && had_detail {
                self.uniform_since_ms = Some(now_ms);
            }
        } else if self.reported != Some(PictureProblem::Covered) {
            // In between: not uniform any more, but a covered picture stays
            // so until it shows something again.
            self.uniform_since_ms = None;
        }

        let covered = self
            .uniform_since_ms
            .is_some_and(|t| now_ms - t >= COVERED_AFTER_MS);
        // A still uniform picture may be a dark scene: not « frozen ».
        let frozen = spread >= UNIFORM_BELOW && now_ms - self.same_since_ms >= FROZEN_AFTER_MS;
        let state = if covered {
            Some(PictureProblem::Covered)
        } else if frozen {
            Some(PictureProblem::Frozen)
        } else {
            None
        };
        if state == self.reported {
            return None;
        }
        self.reported = state;
        Some(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 160;
    const H: usize = 90;

    /// A picture with detail (stripes), its levels moved by `noise`.
    fn scene(seed: &mut u32, noise: u8) -> Vec<u8> {
        (0..W * H)
            .map(|i| {
                *seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let jitter = if noise == 0 {
                    0
                } else {
                    (*seed >> 16) as u8 % (2 * noise + 1)
                };
                let base: u8 = if ((i % W) / 10).is_multiple_of(2) {
                    60
                } else {
                    180
                };
                base.saturating_add(jitter).saturating_sub(noise)
            })
            .collect()
    }

    fn check() -> PictureCheck {
        PictureCheck::new(W, H, &MotionSettings::default())
    }

    /// Feeds `picture(t)` once a second from `from_s` to `to_s`; the changes
    /// reported, with the second they came.
    fn run(
        c: &mut PictureCheck,
        from_s: i64,
        to_s: i64,
        mut picture: impl FnMut(i64) -> Vec<u8>,
    ) -> Vec<(i64, Option<PictureProblem>)> {
        (from_s..to_s)
            .filter_map(|s| c.update(s * 1000, &picture(s)).map(|r| (s, r)))
            .collect()
    }

    #[test]
    fn a_stuck_picture_is_frozen_after_five_minutes_then_back() {
        let mut c = check();
        let mut seed = 1;
        let still = scene(&mut seed, 0);
        let changes = run(&mut c, 0, 400, |_| still.clone());
        assert_eq!(changes, [(300, Some(PictureProblem::Frozen))]);
        let mut seed = 2;
        assert_eq!(c.update(400_000, &scene(&mut seed, 3)), Some(None));
    }

    #[test]
    fn a_real_picture_with_its_noise_is_never_frozen() {
        let mut c = check();
        let mut seed = 7;
        assert!(run(&mut c, 0, 900, |_| scene(&mut seed, 3)).is_empty());
    }

    #[test]
    fn a_dark_still_scene_is_neither_frozen_nor_covered() {
        let mut c = check();
        assert!(run(&mut c, 0, 900, |_| vec![16; W * H]).is_empty());
    }

    #[test]
    fn a_picture_turning_uniform_at_once_is_covered_after_a_minute() {
        let mut c = check();
        let mut seed = 3;
        let changes = run(&mut c, 0, 200, |s| {
            if s < 20 {
                scene(&mut seed, 3)
            } else {
                vec![12; W * H]
            }
        });
        assert_eq!(changes, [(80, Some(PictureProblem::Covered))]);
        let mut seed = 4;
        assert_eq!(c.update(200_000, &scene(&mut seed, 3)), Some(None));
    }

    #[test]
    fn night_falling_and_infrared_switching_are_not_reported() {
        // Ten minutes of dusk: the stripes fade into the dark.
        let mut c = check();
        let mut seed = 5;
        let changes = run(&mut c, 0, 900, |s| {
            let fade = (600 - s.min(600)) as f32 / 600.0;
            scene(&mut seed, 2)
                .into_iter()
                .map(|v| (f32::from(v) * fade) as u8)
                .collect()
        });
        assert!(changes.is_empty(), "{changes:?}");
        // Infrared switching on: a few seconds of grey, then the picture.
        let mut c = check();
        let mut seed = 6;
        let changes = run(&mut c, 0, 300, |s| {
            if (30..50).contains(&s) {
                vec![128; W * H]
            } else {
                scene(&mut seed, 3)
            }
        });
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn one_picture_a_second_at_most() {
        let mut c = check();
        let mut seed = 8;
        let p = scene(&mut seed, 3);
        assert_eq!(c.update(0, &p), None);
        assert_eq!(c.update(500, &p), None);
        assert_eq!(c.last_ms, Some(0));
        assert_eq!(c.update(1000, &p), None);
        assert_eq!(c.last_ms, Some(1000));
        // Another size: ignored.
        assert_eq!(c.update(5000, &p[..100]), None);
    }
}
