//! Crossings of lines: the centre of what moves is followed from one
//! analysed picture to the next; a crossing is counted when the step between
//! two pictures passes over a line, in its allowed direction.
//!
//! One moving thing at a time: with several, the centre lies between them.

use rustnvr_core::{CrossingDirection, CrossingLine};

/// Pictures without motion after which the followed thing is forgotten.
const FORGET_AFTER: u32 = 5;
/// A longer step between two pictures (fraction of the picture) is another
/// thing appearing elsewhere, not a crossing.
const MAX_STEP: f32 = 0.3;
/// Pictures during which a line ignores new crossings after one (the centre
/// wobbling on the line).
const COOLDOWN: u32 = 10;

/// A counted crossing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crossing {
    /// Index of the line (into the settings' lines).
    pub line: usize,
    /// From the left of `a → b` to its right.
    pub forward: bool,
}

/// Follows the centre of motion over the lines of one camera.
pub struct LineTracker {
    lines: Vec<CrossingLine>,
    last: Option<[f32; 2]>,
    quiet: u32,
    cooldown: Vec<u32>,
}

/// `> 0` on the right of `a → b` (picture coordinates, y downwards), `< 0`
/// on its left.
fn side(l: &CrossingLine, p: [f32; 2]) -> f32 {
    (l.b[0] - l.a[0]) * (p[1] - l.a[1]) - (l.b[1] - l.a[1]) * (p[0] - l.a[0])
}

/// Whether the step `from → to` crosses the segment of `l`, and which way.
fn crosses(l: &CrossingLine, from: [f32; 2], to: [f32; 2]) -> Option<bool> {
    let (s1, s2) = (side(l, from), side(l, to));
    if s1 == 0.0 || (s1 > 0.0) == (s2 > 0.0) || s2 == 0.0 {
        return None;
    }
    // The step's line must also separate the ends of the segment.
    let step = CrossingLine {
        name: String::new(),
        a: from,
        b: to,
        direction: CrossingDirection::Both,
    };
    let (t1, t2) = (side(&step, l.a), side(&step, l.b));
    if (t1 > 0.0) == (t2 > 0.0) {
        return None;
    }
    // From the left (negative) to the right (positive): forward.
    Some(s1 < 0.0)
}

impl LineTracker {
    /// Tracker for `lines`.
    pub fn new(lines: &[CrossingLine]) -> Self {
        Self {
            lines: lines.to_vec(),
            last: None,
            quiet: 0,
            cooldown: vec![0; lines.len()],
        }
    }

    /// Whether there is any line to watch.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Name of line `i`.
    pub fn name(&self, i: usize) -> &str {
        self.lines.get(i).map_or("", |l| l.name.as_str())
    }

    /// Next picture: `centre` of the motion, if any. Returns the crossings.
    pub fn update(&mut self, centre: Option<[f32; 2]>) -> Vec<Crossing> {
        for c in &mut self.cooldown {
            *c = c.saturating_sub(1);
        }
        let Some(p) = centre else {
            self.quiet += 1;
            if self.quiet > FORGET_AFTER {
                self.last = None;
            }
            return Vec::new();
        };
        self.quiet = 0;
        let mut out = Vec::new();
        if let Some(q) = self.last
            && (p[0] - q[0]).hypot(p[1] - q[1]) <= MAX_STEP
        {
            for (i, l) in self.lines.iter().enumerate() {
                if self.cooldown[i] > 0 {
                    continue;
                }
                let Some(forward) = crosses(l, q, p) else {
                    continue;
                };
                let wanted = match l.direction {
                    CrossingDirection::Both => true,
                    CrossingDirection::Forward => forward,
                    CrossingDirection::Backward => !forward,
                };
                if wanted {
                    self.cooldown[i] = COOLDOWN;
                    out.push(Crossing { line: i, forward });
                }
            }
        }
        self.last = Some(p);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vertical line in the middle, drawn downwards: its right (forward
    /// side) is the picture's left, so walking to the right is backward.
    fn line(direction: CrossingDirection) -> CrossingLine {
        CrossingLine {
            name: "portail".into(),
            a: [0.5, 0.2],
            b: [0.5, 0.8],
            direction,
        }
    }

    fn walk(t: &mut LineTracker, xs: &[f32], y: f32) -> Vec<Crossing> {
        xs.iter().flat_map(|&x| t.update(Some([x, y]))).collect()
    }

    #[test]
    fn counts_crossings_in_the_allowed_direction() {
        let mut both = LineTracker::new(&[line(CrossingDirection::Both)]);
        let right = walk(&mut both, &[0.3, 0.4, 0.45, 0.55, 0.6], 0.5);
        assert_eq!(right.len(), 1);
        let to_right = right[0].forward;
        // The way back, once the cooldown is over.
        for _ in 0..COOLDOWN {
            both.update(None);
        }
        let back = walk(&mut both, &[0.6, 0.55, 0.45, 0.4], 0.5);
        assert_eq!(back.len(), 1);
        assert_ne!(back[0].forward, to_right);

        // One direction only.
        let wanted = if to_right {
            CrossingDirection::Forward
        } else {
            CrossingDirection::Backward
        };
        let mut one = LineTracker::new(&[line(wanted)]);
        assert_eq!(walk(&mut one, &[0.3, 0.45, 0.55], 0.5).len(), 1);
        for _ in 0..COOLDOWN {
            one.update(None);
        }
        assert!(walk(&mut one, &[0.55, 0.45, 0.3], 0.5).is_empty());
    }

    #[test]
    fn ignores_what_passes_beside_the_line_or_jumps() {
        let mut t = LineTracker::new(&[line(CrossingDirection::Both)]);
        // Below the end of the segment.
        assert!(walk(&mut t, &[0.3, 0.45, 0.55, 0.7], 0.9).is_empty());
        // A jump from one side to the other: another thing, not a crossing.
        let mut t = LineTracker::new(&[line(CrossingDirection::Both)]);
        assert!(walk(&mut t, &[0.1, 0.9], 0.5).is_empty());
        // Wobbling on the line: counted once.
        let mut t = LineTracker::new(&[line(CrossingDirection::Both)]);
        assert_eq!(walk(&mut t, &[0.45, 0.55, 0.45, 0.55, 0.45], 0.5).len(), 1);
        // Forgotten after a pause: no crossing between two separate passages.
        let mut t = LineTracker::new(&[line(CrossingDirection::Both)]);
        t.update(Some([0.4, 0.5]));
        for _ in 0..=FORGET_AFTER {
            t.update(None);
        }
        assert!(t.update(Some([0.6, 0.5])).is_empty());
    }
}
