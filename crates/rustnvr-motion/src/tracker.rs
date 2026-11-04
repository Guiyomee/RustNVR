//! Turns per-picture analyses into events with a start and an end.
//!
//! An event starts after [`START_FRAMES`] consecutive pictures with motion
//! (one noisy picture is not enough), ends once no motion was seen for the
//! configured post duration (its end is the last motion), and is split when
//! it lasts longer than the maximum.

use crate::detector::Analysis;
use std::collections::BTreeSet;

/// Consecutive pictures with motion needed to start an event.
pub const START_FRAMES: u32 = 2;

/// What changed, in milliseconds since the Unix epoch.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// An event started.
    Started {
        /// First picture with motion of the run that started it.
        start_ms: i64,
        /// Score so far.
        score: f32,
        /// Zone indices so far.
        zones: Vec<usize>,
    },
    /// Motion was seen in a zone not reported yet.
    Zones {
        /// Every zone index seen during the event.
        zones: Vec<usize>,
    },
    /// The event ended.
    Ended {
        /// Last picture with motion.
        end_ms: i64,
        /// Largest score of the event.
        score: f32,
        /// Every zone index seen during the event.
        zones: Vec<usize>,
    },
}

struct Active {
    start_ms: i64,
    last_motion_ms: i64,
    score: f32,
    zones: BTreeSet<usize>,
}

/// Event state of one camera.
pub struct Tracker {
    post_ms: i64,
    max_ms: i64,
    run: u32,
    run_start_ms: i64,
    active: Option<Active>,
}

impl Tracker {
    /// Events end after `post_ms` without motion and are split after `max_ms`.
    pub fn new(post_ms: i64, max_ms: i64) -> Self {
        Self {
            post_ms,
            max_ms,
            run: 0,
            run_start_ms: 0,
            active: None,
        }
    }

    /// Whether an event is ongoing.
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    fn end(&mut self) -> Option<Change> {
        let a = self.active.take()?;
        Some(Change::Ended {
            end_ms: a.last_motion_ms,
            score: a.score,
            zones: a.zones.into_iter().collect(),
        })
    }

    /// Takes the analysis of a picture received at `now_ms`.
    pub fn update(&mut self, now_ms: i64, analysis: &Analysis) -> Vec<Change> {
        let mut out = Vec::new();
        out.extend(self.tick(now_ms));
        if !analysis.motion {
            self.run = 0;
            return out;
        }
        if self.run == 0 {
            self.run_start_ms = now_ms;
        }
        self.run = self.run.saturating_add(1);
        if let Some(a) = &mut self.active {
            if now_ms - a.start_ms >= self.max_ms {
                out.extend(self.end());
                self.run = 1;
                self.run_start_ms = now_ms;
            } else {
                a.last_motion_ms = now_ms;
                a.score = a.score.max(analysis.score);
                let before = a.zones.len();
                a.zones.extend(&analysis.zones);
                if a.zones.len() > before {
                    out.push(Change::Zones {
                        zones: a.zones.iter().copied().collect(),
                    });
                }
                return out;
            }
        }
        // A split event restarts at once while motion continues.
        let restart = matches!(out.last(), Some(Change::Ended { .. }));
        if self.run >= START_FRAMES || restart {
            let zones: BTreeSet<usize> = analysis.zones.iter().copied().collect();
            out.push(Change::Started {
                start_ms: self.run_start_ms,
                score: analysis.score,
                zones: zones.iter().copied().collect(),
            });
            self.active = Some(Active {
                start_ms: self.run_start_ms,
                last_motion_ms: now_ms,
                score: analysis.score,
                zones,
            });
        }
        out
    }

    /// Ends the event if no motion was seen for long enough (call regularly,
    /// also when no picture arrives).
    pub fn tick(&mut self, now_ms: i64) -> Option<Change> {
        let idle = self
            .active
            .as_ref()
            .is_some_and(|a| now_ms - a.last_motion_ms >= self.post_ms);
        if idle { self.end() } else { None }
    }

    /// Ends the ongoing event, if any (detection stopping).
    pub fn finish(&mut self) -> Option<Change> {
        self.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motion(zones: &[usize]) -> Analysis {
        Analysis {
            motion: true,
            score: 0.1,
            zones: zones.to_vec(),
            centre: None,
        }
    }

    #[test]
    fn starts_after_consecutive_motion_and_ends_after_post() {
        let mut t = Tracker::new(10_000, 600_000);
        let still = Analysis::default();
        assert!(t.update(0, &motion(&[])).is_empty());
        assert!(
            t.update(200, &still).is_empty(),
            "a single picture is noise"
        );
        assert!(t.update(400, &motion(&[])).is_empty());
        let c = t.update(600, &motion(&[0]));
        assert_eq!(
            c,
            vec![Change::Started {
                start_ms: 400,
                score: 0.1,
                zones: vec![0]
            }]
        );
        assert_eq!(
            t.update(800, &motion(&[1])),
            vec![Change::Zones { zones: vec![0, 1] }]
        );
        assert!(t.update(5_000, &still).is_empty());
        assert!(t.tick(10_700).is_none());
        assert_eq!(
            t.tick(10_800),
            Some(Change::Ended {
                end_ms: 800,
                score: 0.1,
                zones: vec![0, 1]
            })
        );
        assert!(!t.is_active());
    }

    #[test]
    fn splits_long_events() {
        let mut t = Tracker::new(10_000, 60_000);
        t.update(0, &motion(&[]));
        t.update(100, &motion(&[]));
        let mut changes = Vec::new();
        for ms in (200..=61_000).step_by(200) {
            changes.extend(t.update(ms, &motion(&[])));
        }
        assert!(
            matches!(changes[0], Change::Ended { end_ms: 59_800, .. }),
            "{changes:?}"
        );
        assert!(matches!(
            changes[1],
            Change::Started {
                start_ms: 60_000,
                ..
            }
        ));
        assert!(t.finish().is_some());
    }
}
