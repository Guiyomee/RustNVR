//! Night vision seen in the picture: a camera filming in infrared sends a
//! black and white picture. The key frame of a stream, decoded down to a few
//! pixels, says it without AI and almost without processor, whatever the
//! maker of the camera (ONVIF does not tell it in a standard way).

use crate::ffmpeg::{key_frame_input, raw_picture};
use rustnvr_stream::LiveTap;

/// Size of the picture measured, in pixels.
const WIDTH: u32 = 32;
/// See [`WIDTH`].
const HEIGHT: u32 = 18;
/// Mean colour (see [`colourfulness`]) at or below which a picture is black
/// and white: infrared night vision.
pub const GREY_AT_MOST: f32 = 2.5;
/// Mean colour at or above which a picture is in colour; in between, the
/// state does not change (dusk, grey scenes).
pub const COLOUR_AT_LEAST: f32 = 5.0;
/// Readings in a row needed to change state (the first one is enough).
const READINGS_TO_CHANGE: u8 = 2;

/// Mean colour of an `rgb24` picture: the average of max(R, G, B) −
/// min(R, G, B) over its pixels, 0 for black and white, about 15 to 60 for an
/// ordinary picture in colour.
pub fn colourfulness(rgb: &[u8]) -> f32 {
    let (pixels, _) = rgb.as_chunks::<3>();
    let count = pixels.len();
    if count == 0 {
        return 0.0;
    }
    let sum: u64 = pixels
        .iter()
        .map(|p| {
            let (max, min) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
            u64::from(max - min)
        })
        .sum();
    sum as f32 / count as f32
}

/// Mean colour of the current key frame of `source`.
pub async fn measure(ffmpeg: &str, source: &LiveTap) -> Result<f32, String> {
    let input = key_frame_input(source)?;
    let rgb = raw_picture(ffmpeg, WIDTH, HEIGHT, input).await?;
    Ok(colourfulness(&rgb))
}

/// The night vision of one camera, from its readings: it changes after
/// [`READINGS_TO_CHANGE`] readings in a row on the other side of the
/// thresholds.
#[derive(Debug, Default)]
pub struct NightWatch {
    night: Option<bool>,
    /// Readings in a row against the current state.
    against: u8,
}

impl NightWatch {
    /// Whether the camera films in night vision (`None`: not known yet).
    pub fn night(&self) -> Option<bool> {
        self.night
    }

    /// Takes a reading; the new state when it changes.
    pub fn reading(&mut self, colour: f32) -> Option<bool> {
        let vote = if colour <= GREY_AT_MOST {
            true
        } else if colour >= COLOUR_AT_LEAST {
            false
        } else {
            // In between: says nothing, nor breaks a series.
            return None;
        };
        match self.night {
            None => {
                self.night = Some(vote);
                Some(vote)
            }
            Some(now) if now == vote => {
                self.against = 0;
                None
            }
            Some(_) => {
                self.against += 1;
                if self.against < READINGS_TO_CHANGE {
                    return None;
                }
                self.against = 0;
                self.night = Some(vote);
                Some(vote)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_and_white_has_no_colour() {
        assert_eq!(colourfulness(&[90, 90, 90, 200, 200, 200]), 0.0);
        assert_eq!(colourfulness(&[200, 100, 50, 10, 10, 10]), 75.0);
        assert_eq!(colourfulness(&[]), 0.0);
    }

    /// One fragmented MP4 picture of FFmpeg's test pattern (`grey`: in black
    /// and white, as a camera in night vision sends it); `None` without FFmpeg.
    fn pattern(grey: bool) -> Option<Vec<u8>> {
        let filter = if grey {
            "hue=s=0,format=yuv420p"
        } else {
            "format=yuv420p"
        };
        let out = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x180:rate=1",
                "-frames:v",
                "1",
                "-vf",
                filter,
                "-c:v",
                "mpeg4",
                "-f",
                "mp4",
                "-movflags",
                "frag_keyframe+empty_moov",
                "pipe:1",
            ])
            .output()
            .ok()?;
        (out.status.success() && !out.stdout.is_empty()).then_some(out.stdout)
    }

    #[tokio::test]
    async fn a_black_and_white_picture_is_told_from_a_colour_one() {
        let (Some(colour), Some(grey)) = (pattern(false), pattern(true)) else {
            eprintln!("FFmpeg missing: test skipped");
            return;
        };
        let measure = |input| async move {
            colourfulness(
                &raw_picture("ffmpeg", WIDTH, HEIGHT, input)
                    .await
                    .expect("decoded"),
            )
        };
        let (c, g) = (measure(colour).await, measure(grey).await);
        assert!(c >= COLOUR_AT_LEAST, "colour pattern: {c}");
        assert!(g <= GREY_AT_MOST, "black and white pattern: {g}");
    }

    #[test]
    fn night_vision_changes_after_two_readings_in_a_row() {
        let mut w = NightWatch::default();
        assert_eq!(w.night(), None);
        assert_eq!(w.reading(30.0), Some(false), "first reading: day");
        assert_eq!(w.reading(0.5), None, "one grey picture is not enough");
        assert_eq!(
            w.reading(25.0),
            None,
            "back to colour: the series is broken"
        );
        assert_eq!(w.reading(0.5), None);
        assert_eq!(w.reading(3.5), None, "in between: says nothing");
        assert_eq!(w.reading(1.0), Some(true), "two grey readings: night");
        assert_eq!(w.night(), Some(true));
        assert_eq!(w.reading(1.0), None, "no change");
        assert_eq!(w.reading(40.0), None);
        assert_eq!(w.reading(40.0), Some(false), "morning");
    }
}
