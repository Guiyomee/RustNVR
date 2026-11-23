//! Journeys between cameras: the maintainer draws arrows « from → to »
//! between cameras (the order in which one walks past them), and the events
//! where the same kind of object is seen on a camera, then on the next one
//! soon after, are grouped into one journey.
//!
//! Only the arrows are set; any path that follows them is a journey, whole
//! or in part (`Sonnette → Garage → Entrée` and `Sonnette → Entrée` with the
//! arrows `Sonnette → Garage`, `Garage → Entrée`, `Sonnette → Entrée`).

use crate::CameraId;
use serde::{Deserialize, Serialize};

/// At most this many arrows.
pub const MAX_LINKS: usize = 64;
/// Longest delay of an arrow, in seconds.
pub const MAX_DELAY_S: u32 = 600;
/// Delay of an arrow when none is given, in seconds.
pub const DEFAULT_DELAY_S: u32 = 30;

fn default_delay() -> u32 {
    DEFAULT_DELAY_S
}

fn yes() -> bool {
    true
}

/// An arrow: what is seen on `from` may be seen on `to` within
/// `max_delay_s` seconds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JourneyLink {
    /// Camera seen first.
    pub from: String,
    /// Camera seen next.
    pub to: String,
    /// Longest time between the end of the event on `from` and the start
    /// of the one on `to`, in seconds.
    #[serde(default = "default_delay")]
    pub max_delay_s: u32,
    /// Also from `to` to `from` (someone leaving).
    #[serde(default = "yes")]
    pub both_ways: bool,
}

/// The arrows of the installation, the same on every device.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JourneySettings {
    /// Arrows between cameras; empty: no journey.
    #[serde(default)]
    pub links: Vec<JourneyLink>,
}

/// How a step of a journey follows an arrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// `true` in the direction of the arrow, `false` the other way.
    pub forward: bool,
    /// Longest delay of the arrow, in milliseconds.
    pub max_delay_ms: i64,
}

impl JourneySettings {
    /// Checks the arrows against the cameras of the installation; the
    /// message (French) is shown to the user.
    pub fn validate(&self, cameras: &[&str]) -> Result<(), String> {
        if self.links.len() > MAX_LINKS {
            return Err(format!("{MAX_LINKS} flèches au plus"));
        }
        for (i, l) in self.links.iter().enumerate() {
            for camera in [&l.from, &l.to] {
                if !cameras.contains(&camera.as_str()) {
                    return Err(format!("caméra inconnue : {camera:?}"));
                }
            }
            if l.from == l.to {
                return Err(format!("flèche de {:?} vers elle-même", l.from));
            }
            if !(1..=MAX_DELAY_S).contains(&l.max_delay_s) {
                return Err(format!("délai de 1 à {MAX_DELAY_S} s"));
            }
            let same = |o: &JourneyLink| {
                (o.from == l.from && o.to == l.to) || (o.from == l.to && o.to == l.from)
            };
            if self.links[..i].iter().any(same) {
                return Err(format!("flèche {} → {} en double", l.from, l.to));
            }
        }
        Ok(())
    }

    /// Whether `camera` is on some arrow.
    pub fn involves(&self, camera: &str) -> bool {
        self.links
            .iter()
            .any(|l| l.from == camera || l.to == camera)
    }

    /// How going from camera `from` to camera `to` follows an arrow, if one
    /// allows it.
    pub fn step(&self, from: &str, to: &str) -> Option<Step> {
        self.links.iter().find_map(|l| {
            let max_delay_ms = i64::from(l.max_delay_s) * 1000;
            if l.from == from && l.to == to {
                Some(Step {
                    forward: true,
                    max_delay_ms,
                })
            } else if l.both_ways && l.from == to && l.to == from {
                Some(Step {
                    forward: false,
                    max_delay_ms,
                })
            } else {
                None
            }
        })
    }

    /// Longest delay of the arrows leaving `camera` (either way), in
    /// milliseconds; 0 without any.
    pub fn longest_from(&self, camera: &str) -> i64 {
        self.links
            .iter()
            .filter(|l| l.from == camera || (l.both_ways && l.to == camera))
            .map(|l| i64::from(l.max_delay_s) * 1000)
            .max()
            .unwrap_or(0)
    }

    /// The arrows without the cameras that are no longer there.
    pub fn without_missing(&self, cameras: &[&str]) -> Self {
        Self {
            links: self
                .links
                .iter()
                .filter(|l| cameras.contains(&l.from.as_str()) && cameras.contains(&l.to.as_str()))
                .cloned()
                .collect(),
        }
    }
}

/// Which way a whole journey went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JourneyDirection {
    /// Every step in the direction of the arrows (someone arriving).
    Arrival,
    /// Every step against them (someone leaving).
    Departure,
}

/// The direction of a journey from the way each of its steps went; `None`
/// when they disagree.
pub fn direction(steps: &[bool]) -> Option<JourneyDirection> {
    if steps.is_empty() {
        None
    } else if steps.iter().all(|f| *f) {
        Some(JourneyDirection::Arrival)
    } else if steps.iter().all(|f| !*f) {
        Some(JourneyDirection::Departure)
    } else {
        None
    }
}

/// One camera of a journey: the event seen there.
#[derive(Clone, Debug, PartialEq)]
pub struct JourneyStep {
    /// Event identifier.
    pub event: i64,
    /// Camera.
    pub camera: CameraId,
    /// Start of the event, ms since the Unix epoch.
    pub start_ms: i64,
    /// End of the event; `None` while ongoing.
    pub end_ms: Option<i64>,
}

/// A journey that grew (one more camera) or ended, on the event bus.
#[derive(Clone, Debug, PartialEq)]
pub struct JourneyUpdate {
    /// Identifier: that of the event that started it (usually its first;
    /// an event the AI recognized later may come before it).
    pub id: i64,
    /// Kind of object followed (`person`, `car`…).
    pub label: String,
    /// The cameras in order, two at least.
    pub steps: Vec<JourneyStep>,
    /// Which way it went, when every step agrees.
    pub direction: Option<JourneyDirection>,
    /// No camera can join it any more.
    pub ended: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(from: &str, to: &str) -> JourneyLink {
        JourneyLink {
            from: from.into(),
            to: to.into(),
            max_delay_s: 30,
            both_ways: true,
        }
    }

    #[test]
    fn follows_the_arrows_both_ways() {
        let mut s = JourneySettings {
            links: vec![
                link("sonnette", "garage"),
                link("sonnette", "entree"),
                link("garage", "entree"),
                JourneyLink {
                    max_delay_s: 60,
                    both_ways: false,
                    ..link("entree", "salon")
                },
            ],
        };
        let cams = ["sonnette", "garage", "entree", "salon"];
        assert_eq!(s.validate(&cams), Ok(()));
        assert_eq!(
            s.step("sonnette", "entree"),
            Some(Step {
                forward: true,
                max_delay_ms: 30_000
            })
        );
        assert_eq!(s.step("entree", "garage").map(|x| x.forward), Some(false));
        assert_eq!(s.step("salon", "entree"), None, "one way only");
        assert_eq!(s.step("sonnette", "salon"), None, "no arrow");
        assert_eq!(s.longest_from("entree"), 60_000);
        assert_eq!(s.longest_from("salon"), 0);
        assert!(s.involves("salon") && !s.involves("jardin"));
        assert_eq!(s.without_missing(&["sonnette", "garage"]).links.len(), 1);

        s.links.push(link("entree", "sonnette"));
        assert!(s.validate(&cams).is_err_and(|e| e.contains("double")));
        s.links.pop();
        s.links.push(link("salon", "salon"));
        assert!(s.validate(&cams).is_err());
        s.links.pop();
        s.links.push(link("salon", "jardin"));
        assert!(s.validate(&cams).is_err_and(|e| e.contains("inconnue")));
        s.links.pop();
        s.links[0].max_delay_s = 0;
        assert!(s.validate(&cams).is_err());
    }

    #[test]
    fn names_the_direction_and_reads_defaults() {
        assert_eq!(direction(&[true, true]), Some(JourneyDirection::Arrival));
        assert_eq!(direction(&[false]), Some(JourneyDirection::Departure));
        assert_eq!(direction(&[true, false]), None);
        assert_eq!(direction(&[]), None);
        let s: JourneySettings =
            toml::from_str("[[links]]\nfrom = \"a\"\nto = \"b\"\n").expect("toml");
        assert_eq!(s.links[0].max_delay_s, DEFAULT_DELAY_S);
        assert!(s.links[0].both_ways);
        assert!(toml::from_str::<JourneySettings>("links = []\nx = 1\n").is_err());
    }
}
