//! Motion detection settings of a camera and detection events.

use crate::camera::CameraId;
use serde::{Deserialize, Serialize};

/// Most zones per camera.
pub const MAX_ZONES: usize = 8;
/// Most points per zone.
pub const MAX_ZONE_POINTS: usize = 32;
/// Most privacy masks per camera.
pub const MAX_MASKS: usize = 8;

/// A rectangle of the picture hidden from view (privacy): blacked out in
/// the live view, playback, thumbnails, snapshots and exports, and ignored by
/// motion detection. Recordings keep the full picture (no re-encoding).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivacyMask {
    /// Name shown in the interface.
    pub name: String,
    /// `[x1, y1, x2, y2]` in fractions of the picture.
    pub rect: [f32; 4],
}

/// Motion detection settings of one camera, edited in the web interface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionSettings {
    /// Analyse this camera. Off by default: decoding costs CPU.
    pub enabled: bool,
    /// 1 (only large, contrasted changes) to 100 (the slightest change),
    /// used for the whole picture when there is no zone.
    pub sensitivity: u8,
    /// Areas where motion counts. Empty: the whole picture.
    #[serde(default)]
    pub zones: Vec<MotionZone>,
    /// Objects to recognize for this camera (optional AI), among the labels
    /// of `[ai]`; empty: all of them.
    #[serde(default)]
    pub objects: Vec<String>,
    /// Areas hidden from view (privacy masks).
    #[serde(default)]
    pub masks: Vec<PrivacyMask>,
    /// Lines whose crossings are counted.
    #[serde(default)]
    pub lines: Vec<CrossingLine>,
}

/// Most crossing lines per camera.
pub const MAX_LINES: usize = 4;

/// Which crossings of a line count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossingDirection {
    /// Both ways.
    #[default]
    Both,
    /// From the left of `a → b` to its right (the arrow of the interface).
    Forward,
    /// The other way.
    Backward,
}

/// A line of the picture whose crossings are counted (from one picture to
/// the next, the centre of what moves passes over it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossingLine {
    /// Name shown in the interface and in MQTT.
    pub name: String,
    /// First end, in fractions of the picture.
    pub a: [f32; 2],
    /// Second end.
    pub b: [f32; 2],
    /// Crossings counted.
    #[serde(default)]
    pub direction: CrossingDirection,
}

impl Default for MotionSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            sensitivity: 50,
            zones: Vec::new(),
            objects: Vec::new(),
            masks: Vec::new(),
            lines: Vec::new(),
        }
    }
}

/// A polygonal area of the picture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionZone {
    /// Display name, reported in events.
    pub name: String,
    /// Polygon corners, as fractions of the picture width and height (0..1),
    /// so that zones survive resolution changes.
    pub points: Vec<[f32; 2]>,
    /// Sensitivity in this zone, like [`MotionSettings::sensitivity`].
    pub sensitivity: u8,
    /// Recognized objects (optional AI) that count only inside this zone or
    /// the other zones listing them, e.g. `["person"]` for a path.
    #[serde(default)]
    pub objects: Vec<String>,
}

impl MotionZone {
    /// Whether the point `(x, y)` (fractions of the picture) is inside the
    /// zone (even-odd rule).
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let poly = &self.points;
        if poly.len() < 3 {
            return false;
        }
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
}

impl MotionSettings {
    /// Whether a recognized object counts for this camera, with the zone it
    /// is in: `None` when it does not count. An object is placed at the
    /// bottom middle of its box (where a person stands). If its label is
    /// chosen in some zones, it counts only inside one of them; otherwise it
    /// counts anywhere. Labels not in [`Self::objects`] (when not empty) never
    /// count.
    pub fn locate(&self, o: &DetectedObject) -> Option<Option<String>> {
        if !self.objects.is_empty() && !self.objects.contains(&o.label) {
            return None;
        }
        let point = o.bbox.map(|b| ((b[0] + b[2]) / 2.0, b[3]));
        let inside = |z: &&MotionZone| point.is_some_and(|(x, y)| z.contains(x, y));
        let restricted: Vec<&MotionZone> = self
            .zones
            .iter()
            .filter(|z| z.objects.contains(&o.label))
            .collect();
        if restricted.is_empty() {
            Some(self.zones.iter().find(inside).map(|z| z.name.clone()))
        } else {
            restricted
                .into_iter()
                .find(inside)
                .map(|z| Some(z.name.clone()))
        }
    }
}

impl MotionSettings {
    /// Every problem of these settings (empty when valid).
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if !(1..=100).contains(&self.sensitivity) {
            p.push("la sensibilité doit être comprise entre 1 et 100".to_owned());
        }
        if self.zones.len() > MAX_ZONES {
            p.push(format!("{MAX_ZONES} zones au maximum"));
        }
        for z in &self.zones {
            if z.name.trim().is_empty() || z.name.len() > 64 {
                p.push("chaque zone doit avoir un nom (64 caractères au plus)".to_owned());
            }
            if !(3..=MAX_ZONE_POINTS).contains(&z.points.len()) {
                p.push(format!(
                    "zone {:?} : il faut entre 3 et {MAX_ZONE_POINTS} points",
                    z.name
                ));
            }
            if z.points.iter().flatten().any(|v| !(0.0..=1.0).contains(v)) {
                p.push(format!(
                    "zone {:?} : coordonnées hors de l'image (0 à 1)",
                    z.name
                ));
            }
            if !(1..=100).contains(&z.sensitivity) {
                p.push(format!(
                    "zone {:?} : la sensibilité doit être comprise entre 1 et 100",
                    z.name
                ));
            }
        }
        if self.masks.len() > MAX_MASKS {
            p.push(format!("{MAX_MASKS} masques au maximum"));
        }
        for m in &self.masks {
            let [x1, y1, x2, y2] = m.rect;
            if m.name.trim().is_empty() || m.name.len() > 64 {
                p.push("chaque masque doit avoir un nom (64 caractères au plus)".to_owned());
            }
            if m.rect.iter().any(|v| !(0.0..=1.0).contains(v)) || x2 <= x1 || y2 <= y1 {
                p.push(format!(
                    "masque {:?} : rectangle invalide (0 à 1, fin après le début)",
                    m.name
                ));
            }
        }
        if self.lines.len() > MAX_LINES {
            p.push(format!("{MAX_LINES} lignes de passage au maximum"));
        }
        for l in &self.lines {
            if l.name.trim().is_empty() || l.name.len() > 64 {
                p.push("chaque ligne doit avoir un nom (64 caractères au plus)".to_owned());
            }
            let outside = l.a.iter().chain(&l.b).any(|v| !(0.0..=1.0).contains(v));
            let short = (l.a[0] - l.b[0]).hypot(l.a[1] - l.b[1]) < 0.02;
            if outside || short {
                p.push(format!(
                    "ligne {:?} : extrémités dans l'image (0 à 1) et assez éloignées",
                    l.name
                ));
            }
        }
        let mut names: Vec<&str> = self.lines.iter().map(|l| l.name.trim()).collect();
        names.sort_unstable();
        if names.windows(2).any(|w| w[0] == w[1]) {
            p.push("deux lignes de passage portent le même nom".to_owned());
        }
        p
    }

    /// The masks as `[x1, y1, x2, y2]` rectangles.
    pub fn mask_rects(&self) -> Vec<[f32; 4]> {
        self.masks.iter().map(|m| m.rect).collect()
    }

    /// Whether the point `(x, y)` (picture fractions) is hidden by a mask.
    pub fn masked(&self, x: f32, y: f32) -> bool {
        self.masks
            .iter()
            .any(|m| x >= m.rect[0] && x <= m.rect[2] && y >= m.rect[1] && y <= m.rect[3])
    }
}

/// What was detected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Changes in the picture (no object recognition).
    Motion,
}

impl EventKind {
    /// Name stored in the database and sent by the API.
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Motion => "motion",
        }
    }
}

/// An object recognized in an event picture by the optional AI provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DetectedObject {
    /// Label given by the provider (`person`, `car`…).
    pub label: String,
    /// Confidence, 0..1.
    pub score: f32,
    /// Box as fractions of the picture: `[x_min, y_min, x_max, y_max]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f32; 4]>,
    /// Detection zone the object is in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
}

/// A detection event, ongoing (`end_ms` is `None`) or finished.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectionEvent {
    /// Database identifier.
    pub id: i64,
    /// Camera.
    pub camera: CameraId,
    /// What was detected.
    pub kind: EventKind,
    /// Start, in milliseconds since the Unix epoch.
    pub start_ms: i64,
    /// End, in milliseconds since the Unix epoch; `None` while ongoing.
    pub end_ms: Option<i64>,
    /// Largest fraction of a zone (or of the picture) that changed, 0..1.
    pub score: f32,
    /// Zones where motion was seen (empty without zones).
    pub zones: Vec<String>,
    /// Whether a thumbnail picture exists.
    pub has_thumbnail: bool,
    /// Objects recognized in the thumbnail (optional AI; empty without it).
    pub objects: Vec<DetectedObject>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_objects_in_zones() {
        // Path on the left half of the picture, for persons only.
        let settings = MotionSettings {
            enabled: true,
            sensitivity: 50,
            zones: vec![MotionZone {
                name: "Allée".into(),
                points: vec![[0.0, 0.5], [0.5, 0.5], [0.5, 1.0], [0.0, 1.0]],
                sensitivity: 50,
                objects: vec!["person".into()],
            }],
            objects: Vec::new(),
            masks: Vec::new(),
            lines: Vec::new(),
        };
        let object = |label: &str, bbox: [f32; 4]| DetectedObject {
            label: label.into(),
            score: 0.9,
            bbox: Some(bbox),
            zone: None,
        };
        // Feet in the path: counts, in « Allée ».
        assert_eq!(
            settings.locate(&object("person", [0.1, 0.2, 0.3, 0.8])),
            Some(Some("Allée".into()))
        );
        // Head over the hedge but feet on the pavement (right half): no.
        assert_eq!(
            settings.locate(&object("person", [0.6, 0.2, 0.8, 0.9])),
            None
        );
        // A car is not restricted by the path: counts anywhere.
        assert_eq!(
            settings.locate(&object("car", [0.6, 0.2, 0.9, 0.9])),
            Some(None)
        );
        // Objects of the camera: a car is not wanted at all.
        let persons_only = MotionSettings {
            objects: vec!["person".into()],
            ..settings
        };
        assert_eq!(
            persons_only.locate(&object("car", [0.1, 0.6, 0.3, 0.9])),
            None
        );
    }

    #[test]
    fn checks_and_applies_masks() {
        let mask = |rect| PrivacyMask {
            name: "rue".into(),
            rect,
        };
        let s = MotionSettings {
            masks: vec![mask([0.6, 0.0, 1.0, 0.3])],
            ..MotionSettings::default()
        };
        assert!(s.problems().is_empty());
        assert!(s.masked(0.8, 0.1) && !s.masked(0.5, 0.1) && !s.masked(0.8, 0.5));
        assert_eq!(s.mask_rects(), vec![[0.6, 0.0, 1.0, 0.3]]);
        let bad = MotionSettings {
            masks: vec![mask([0.5, 0.5, 0.4, 0.9]), mask([0.0, 0.0, 1.2, 1.0])],
            ..MotionSettings::default()
        };
        assert_eq!(bad.problems().len(), 2, "{:?}", bad.problems());
    }

    #[test]
    fn validates_settings() {
        assert!(MotionSettings::default().problems().is_empty());
        let bad = MotionSettings {
            enabled: true,
            sensitivity: 0,
            zones: vec![MotionZone {
                name: " ".into(),
                points: vec![[0.0, 0.0], [1.5, 0.0]],
                sensitivity: 101,
                objects: Vec::new(),
            }],
            objects: Vec::new(),
            masks: Vec::new(),
            lines: Vec::new(),
        };
        assert_eq!(bad.problems().len(), 5, "{:?}", bad.problems());
    }
}
