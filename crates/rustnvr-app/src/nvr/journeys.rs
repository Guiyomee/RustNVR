//! Journeys between cameras: events where the AI recognized the same kind
//! of object, on cameras joined by an arrow (`rustnvr_core::journey`) and
//! close enough in time, are grouped into one journey.
//!
//! A journey is a chain of events in the order they started, each camera
//! joined to the next by an arrow. The AI does not answer in that order (a
//! person recognized at the 4th picture on one camera, at the 1st on the
//! next): an event takes its place by its start, at the end, at the head or
//! in the middle of a journey. When two journeys could take it (two people
//! at once), it joins none: a wrong journey is worse than none.

use super::Shared;
use rustnvr_core::CameraId;
use rustnvr_core::journey::{JourneySettings, JourneyStep, JourneyUpdate, direction};
use rustnvr_core::{DetectionEvent, NvrEvent};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// How often the journeys that can no longer grow are closed.
const CLOSE_EVERY: Duration = Duration::from_secs(5);
/// A journey whose last event lasts longer is closed anyway.
const LONGEST_STEP_MS: i64 = 3600 * 1000;
/// Events placed are remembered up to this many.
const REMEMBERED: usize = 2000;

/// A journey being followed (one event or more).
#[derive(Debug)]
struct Open {
    /// Identifier: its first event placed, which stays when an event that
    /// started earlier comes later.
    id: i64,
    /// In the order they started.
    steps: Vec<JourneyStep>,
    /// The kinds of object seen on every camera so far.
    labels: Vec<String>,
}

impl Open {
    fn update(&self, settings: &JourneySettings, ended: bool) -> JourneyUpdate {
        let forward: Vec<bool> = self
            .steps
            .windows(2)
            .map(|w| {
                settings
                    .step(w[0].camera.as_str(), w[1].camera.as_str())
                    .is_none_or(|s| s.forward)
            })
            .collect();
        JourneyUpdate {
            id: self.id,
            label: self.labels.first().cloned().unwrap_or_default(),
            steps: self.steps.clone(),
            direction: direction(&forward),
            ended,
        }
    }

    /// Where event `step` would go in this journey: the index of its place,
    /// if arrows join it to the events before and after it in time.
    fn place(&self, step: &JourneyStep, settings: &JourneySettings, now_ms: i64) -> Option<usize> {
        if self.steps.iter().any(|s| s.camera == step.camera) {
            return None;
        }
        let at = self.steps.partition_point(|s| s.start_ms <= step.start_ms);
        let joined =
            |a: &JourneyStep, b: &JourneyStep| follows(settings, a, &b.camera, b.start_ms, now_ms);
        let after_previous = at == 0 || joined(&self.steps[at - 1], step);
        let before_next = self.steps.get(at).is_none_or(|next| joined(step, next));
        (after_previous && before_next).then_some(at)
    }
}

/// Whether what was seen at `from` can be seen next on `camera` at
/// `start_ms`: an arrow joins them, and it started soon enough after the
/// end of `from` (or now, while `from` lasts).
fn follows(
    settings: &JourneySettings,
    from: &JourneyStep,
    camera: &CameraId,
    start_ms: i64,
    now_ms: i64,
) -> bool {
    settings
        .step(from.camera.as_str(), camera.as_str())
        .is_some_and(|step| {
            start_ms <= from.end_ms.unwrap_or(now_ms).max(from.start_ms) + step.max_delay_ms
        })
}

/// What the linker asks for.
#[derive(Debug, PartialEq)]
pub(super) enum Action {
    /// Store `events` as part of journey `journey`.
    Store { events: Vec<i64>, journey: i64 },
    /// Tell the others (WebSocket, notifications, MQTT).
    Announce(JourneyUpdate),
}

/// Groups events into journeys; pure, the clock is given.
#[derive(Debug, Default)]
pub(super) struct Linker {
    open: Vec<Open>,
    placed: HashSet<i64>,
}

impl Linker {
    /// An event started, changed or ended.
    pub(super) fn event(
        &mut self,
        e: &DetectionEvent,
        settings: &JourneySettings,
        now_ms: i64,
    ) -> Vec<Action> {
        // Its end: that of its step.
        if let Some(end) = e.end_ms {
            for open in &mut self.open {
                if let Some(step) = open.steps.iter_mut().find(|s| s.event == e.id) {
                    step.end_ms = Some(end);
                }
            }
        }
        if e.objects.is_empty() || self.placed.contains(&e.id) {
            return Vec::new();
        }
        if self.placed.len() > REMEMBERED {
            self.placed.clear();
        }
        self.placed.insert(e.id);
        let camera = e.camera.as_str();
        if !settings.involves(camera) {
            return Vec::new();
        }
        let labels: Vec<String> = e.objects.iter().map(|o| o.label.clone()).collect();
        let step = JourneyStep {
            event: e.id,
            camera: e.camera.clone(),
            start_ms: e.start_ms,
            end_ms: e.end_ms,
        };
        let candidates: Vec<(usize, usize)> = self
            .open
            .iter()
            .enumerate()
            .filter(|(_, open)| open.labels.iter().any(|l| labels.contains(l)))
            .filter_map(|(i, open)| Some((i, open.place(&step, settings, now_ms)?)))
            .collect();
        let [(i, at)] = candidates[..] else {
            if candidates.len() > 1 {
                debug!(camera = %e.camera, id = e.id, journeys = candidates.len(), "event fits several journeys: joins none");
            }
            // It may start one.
            self.open.push(Open {
                id: e.id,
                steps: vec![step],
                labels,
            });
            return Vec::new();
        };
        let open = &mut self.open[i];
        open.steps.insert(at, step);
        open.labels.retain(|l| labels.contains(l));
        let events = if open.steps.len() == 2 {
            open.steps.iter().map(|s| s.event).collect()
        } else {
            vec![e.id]
        };
        vec![
            Action::Store {
                events,
                journey: open.id,
            },
            Action::Announce(open.update(settings, false)),
        ]
    }

    /// Closes the journeys that can no longer grow; those of two cameras or
    /// more are announced as ended.
    pub(super) fn close(&mut self, settings: &JourneySettings, now_ms: i64) -> Vec<JourneyUpdate> {
        let mut ended = Vec::new();
        self.open.retain(|open| {
            let Some(last) = open.steps.last() else {
                return false;
            };
            let over = match last.end_ms {
                Some(end) => now_ms > end + settings.longest_from(last.camera.as_str()),
                None => now_ms > last.start_ms + LONGEST_STEP_MS,
            };
            if over && open.steps.len() > 1 {
                ended.push(open.update(settings, true));
            }
            !over
        });
        ended
    }
}

pub(super) fn settings(shared: &Shared) -> JourneySettings {
    shared.api.get().map(|a| a.journeys()).unwrap_or_default()
}

/// Follows the events and groups them into journeys.
pub(super) async fn run(shared: Arc<Shared>, cancel: CancellationToken) {
    let Some(db) = shared.database.clone() else {
        return;
    };
    let mut events = shared.engine.events.subscribe();
    let mut linker = Linker::default();
    let mut ticker = tokio::time::interval(CLOSE_EVERY);
    loop {
        let now_ms = || jiff::Timestamp::now().as_millisecond();
        let actions = tokio::select! {
            () = cancel.cancelled() => return,
            _ = ticker.tick() => linker
                .close(&settings(&shared), now_ms())
                .into_iter()
                .map(Action::Announce)
                .collect(),
            e = events.recv() => match e {
                Ok(NvrEvent::Detection(e)) => linker.event(&e, &settings(&shared), now_ms()),
                Ok(_) => continue,
                Err(RecvError::Lagged(n)) => {
                    warn!(skipped = n, "journeys lagging behind the events");
                    continue;
                }
                Err(RecvError::Closed) => return,
            },
        };
        for action in actions {
            match action {
                Action::Store { events, journey } => {
                    if let Err(e) = db.set_journey(&events, journey).await {
                        warn!(journey, error = %e, "journey not stored");
                    }
                }
                Action::Announce(update) => {
                    let cameras: Vec<&str> =
                        update.steps.iter().map(|s| s.camera.as_str()).collect();
                    if update.ended {
                        info!(journey = update.id, object = %update.label, cameras = ?cameras, "journey ended");
                    } else {
                        debug!(journey = update.id, object = %update.label, cameras = ?cameras, "journey grew");
                    }
                    let _ = shared.engine.events.send(NvrEvent::Journey(update));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnvr_core::journey::{JourneyDirection, JourneyLink};
    use rustnvr_core::{CameraId, DetectedObject, EventKind};

    fn link(from: &str, to: &str) -> JourneyLink {
        JourneyLink {
            from: from.into(),
            to: to.into(),
            max_delay_s: 30,
            both_ways: true,
        }
    }

    /// Sonnette → Garage → Entrée → Salon, and Sonnette → Entrée.
    fn house() -> JourneySettings {
        JourneySettings {
            links: vec![
                link("sonnette", "garage"),
                link("sonnette", "entree"),
                link("garage", "entree"),
                link("entree", "salon"),
            ],
        }
    }

    fn event(
        id: i64,
        camera: &str,
        start_s: i64,
        end_s: Option<i64>,
        label: &str,
    ) -> DetectionEvent {
        DetectionEvent {
            id,
            camera: CameraId::new(camera).expect("id"),
            kind: EventKind::Motion,
            start_ms: start_s * 1000,
            end_ms: end_s.map(|s| s * 1000),
            score: 0.1,
            zones: Vec::new(),
            has_thumbnail: true,
            objects: vec![DetectedObject {
                label: label.into(),
                score: 0.9,
                bbox: None,
                zone: None,
                plate: None,
                plate_owner: None,
            }],
            journey: None,
        }
    }

    fn cameras(u: &JourneyUpdate) -> Vec<&str> {
        u.steps.iter().map(|s| s.camera.as_str()).collect()
    }

    #[test]
    fn follows_someone_arriving() {
        let s = house();
        let mut l = Linker::default();
        assert!(
            l.event(&event(1, "sonnette", 0, Some(6), "person"), &s, 6_000)
                .is_empty()
        );
        let a = l.event(&event(2, "entree", 10, None, "person"), &s, 11_000);
        assert_eq!(
            a[0],
            Action::Store {
                events: vec![1, 2],
                journey: 1
            }
        );
        let a = l.event(&event(3, "salon", 15, None, "person"), &s, 16_000);
        assert_eq!(
            a[0],
            Action::Store {
                events: vec![3],
                journey: 1
            }
        );
        let Action::Announce(u) = &a[1] else {
            panic!("announce")
        };
        assert_eq!(cameras(u), ["sonnette", "entree", "salon"]);
        assert_eq!(u.direction, Some(JourneyDirection::Arrival));
        assert!(!u.ended);
        // The salon event ends at 20 s: closed 30 s later (longest arrow).
        l.event(&event(3, "salon", 15, Some(20), "person"), &s, 20_000);
        assert!(l.close(&s, 45_000).is_empty());
        let ended = l.close(&s, 51_000);
        assert_eq!(ended.len(), 1);
        assert!(ended[0].ended);
        assert_eq!(ended[0].steps[2].end_ms, Some(20_000));
        assert!(l.open.is_empty());
    }

    #[test]
    fn follows_the_garage_path_and_someone_leaving() {
        let s = house();
        let mut l = Linker::default();
        l.event(&event(1, "sonnette", 0, Some(3), "person"), &s, 3_000);
        l.event(&event(2, "garage", 8, Some(20), "person"), &s, 20_000);
        let a = l.event(&event(3, "entree", 40, None, "person"), &s, 40_000);
        let Action::Announce(u) = &a[1] else {
            panic!("announce")
        };
        assert_eq!(cameras(u), ["sonnette", "garage", "entree"]);

        let mut l = Linker::default();
        l.event(&event(1, "salon", 0, Some(3), "person"), &s, 3_000);
        let a = l.event(&event(2, "entree", 5, Some(8), "person"), &s, 8_000);
        let Action::Announce(u) = &a[1] else {
            panic!("announce")
        };
        assert_eq!(u.direction, Some(JourneyDirection::Departure));
    }

    #[test]
    fn leaves_out_what_does_not_fit() {
        let s = house();
        let mut l = Linker::default();
        // Too late (30 s arrow), another kind of object, no arrow.
        l.event(&event(1, "sonnette", 0, Some(5), "person"), &s, 5_000);
        assert!(
            l.event(&event(2, "entree", 40, None, "person"), &s, 40_000)
                .is_empty()
        );
        assert!(
            l.event(&event(3, "garage", 10, None, "car"), &s, 10_000)
                .is_empty()
        );
        assert!(
            l.event(&event(4, "jardin", 6, None, "person"), &s, 6_000)
                .is_empty()
        );
        // Without objects: not placed, the AI may still answer.
        let mut bare = event(5, "garage", 7, None, "person");
        bare.objects.clear();
        assert!(l.event(&bare, &s, 7_000).is_empty());
        assert!(
            !l.event(&event(5, "garage", 7, None, "person"), &s, 8_000)
                .is_empty()
        );
        // An event is placed once.
        assert!(
            l.event(&event(5, "garage", 7, None, "person"), &s, 9_000)
                .is_empty()
        );
    }

    #[test]
    fn places_an_event_the_ai_recognized_late() {
        // Seen on a real install, 21/11: garage at 10:50:51 recognized at
        // the 4th picture (10:51:10), after the doorbell (10:50:55,
        // recognized at 10:50:58); then the entrance and the living room.
        let s = JourneySettings {
            links: vec![
                link("sonnette", "entree"),
                link("entree", "salon"),
                link("sonnette", "garage"),
                link("garage", "entree"),
            ],
        };
        let mut l = Linker::default();
        assert!(
            l.event(&event(47, "sonnette", 55, None, "person"), &s, 58_000)
                .is_empty()
        );
        // The garage started before: it goes at the head.
        let a = l.event(&event(46, "garage", 51, None, "person"), &s, 70_000);
        assert_eq!(
            a[0],
            Action::Store {
                events: vec![46, 47],
                journey: 47
            }
        );
        l.event(&event(47, "sonnette", 55, Some(65), "person"), &s, 70_000);
        // The entrance: one journey only can take it (no second one).
        let a = l.event(&event(48, "entree", 74, None, "person"), &s, 91_000);
        assert_eq!(
            a[0],
            Action::Store {
                events: vec![48],
                journey: 47
            }
        );
        let a = l.event(&event(49, "salon", 90, None, "person"), &s, 103_000);
        let Action::Announce(u) = &a[1] else {
            panic!("announce")
        };
        assert_eq!(cameras(u), ["garage", "sonnette", "entree", "salon"]);
        assert_eq!(u.id, 47);
        // Garage → doorbell goes against its arrow, the rest along them.
        assert_eq!(u.direction, None);

        // In the middle: doorbell and entrance first, the garage between.
        let mut l = Linker::default();
        l.event(&event(1, "sonnette", 0, Some(3), "person"), &s, 3_000);
        l.event(&event(3, "entree", 20, None, "person"), &s, 21_000);
        let a = l.event(&event(2, "garage", 8, Some(15), "person"), &s, 25_000);
        let Action::Announce(u) = &a[1] else {
            panic!("announce")
        };
        assert_eq!(cameras(u), ["sonnette", "garage", "entree"]);
        assert_eq!(u.direction, Some(JourneyDirection::Arrival));

        // Too early for the head: ended more than 30 s before the next camera.
        let mut l = Linker::default();
        l.event(&event(1, "sonnette", 100, None, "person"), &s, 101_000);
        assert!(
            l.event(&event(2, "garage", 10, Some(60), "person"), &s, 102_000)
                .is_empty()
        );
    }

    #[test]
    fn joins_no_journey_when_two_could_take_it() {
        let s = house();
        let mut l = Linker::default();
        // Two people at the door, one after the other: two journeys.
        l.event(&event(1, "sonnette", 0, Some(5), "person"), &s, 5_000);
        l.event(&event(2, "sonnette", 2, Some(6), "person"), &s, 6_000);
        assert_eq!(l.open.len(), 2);
        // Then someone at the entrance: which one?
        assert!(
            l.event(&event(3, "entree", 8, None, "person"), &s, 8_000)
                .is_empty()
        );
        // Nothing of one camera only is announced at its end.
        assert!(l.close(&s, 3_600_000 * 2).is_empty());
    }
}
