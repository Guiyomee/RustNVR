//! Journeys between cameras (see `rustnvr_core::journey`): the arrows,
//! chosen by the administrators, and the journeys found by the application
//! (events grouped by `journey`).

use crate::dto::EventDto;
use crate::routes::ApiError;
use crate::state::ApiState;
use axum::Json;
use axum::extract::{Path, State};
use rustnvr_core::DetectionEvent;
use rustnvr_core::journey::{JourneyDirection, JourneySettings, direction};
use serde::Serialize;
use std::sync::Arc;

fn database(state: &ApiState) -> Result<&rustnvr_storage::Database, ApiError> {
    state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("events are not available on this server"))
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %e, "database error");
    ApiError::internal(e.to_string())
}

/// What a journey video shows of each of its cameras: `(index of the step,
/// from, to)` in ms since the Unix epoch. `before_ms` before the first
/// camera, the next one as soon as it sees the object, `after_ms` after the
/// last; at most `max_part_ms` per camera and `max_total_ms` in all.
/// `steps`: start and end of each event, in order.
pub fn video_periods(
    steps: &[(i64, Option<i64>)],
    before_ms: i64,
    after_ms: i64,
    max_part_ms: i64,
    max_total_ms: i64,
) -> Vec<(usize, i64, i64)> {
    /// A camera is shown 1 s at least.
    const SHORTEST_MS: i64 = 1_000;
    let mut out = Vec::new();
    let mut total = 0;
    for (i, (start, end)) in steps.iter().enumerate() {
        let from = if i == 0 { start - before_ms } else { *start };
        let to = match steps.get(i + 1) {
            Some((next, _)) => *next,
            None => end.unwrap_or(*start) + after_ms,
        };
        let length = (to - from)
            .clamp(SHORTEST_MS, max_part_ms.max(SHORTEST_MS))
            .min(max_total_ms - total);
        if length < SHORTEST_MS {
            break;
        }
        total += length;
        out.push((i, from, from + length));
    }
    out
}

/// The FFmpeg filter graph of a journey video: each input `i` (a concat
/// list) cut to `(offset, length)` with `trim` (several concat inputs cannot
/// seek), masked (`masks`, may be empty), brought to `size` at `fps`, with
/// the banner of text file `t<i>.txt` (current folder) when `banners`, then
/// the inputs one after the other, as `[out]`. With sound (the last field of
/// a part: its recordings have an audio track) on one part at least, the
/// sounds follow too, as `[aout]` (48 kHz mono): the camera's own, cut the
/// same way, or silence as long as its picture for a part without sound.
pub fn video_graph(
    parts: &[(&str, i64, i64, bool)],
    (w, h): (u32, u32),
    fps: u32,
    banners: bool,
) -> String {
    let seconds = |ms: i64| format!("{}.{:03}", ms / 1000, ms % 1000);
    let font = h / 21;
    let mut graph = String::new();
    let sound = parts.iter().any(|p| p.3);
    for (i, (masks, offset_ms, length_ms, audio)) in parts.iter().enumerate() {
        let mut f = vec![format!(
            "trim=start={}:duration={},setpts=PTS-STARTPTS",
            seconds(*offset_ms),
            seconds(*length_ms)
        )];
        if !masks.is_empty() {
            f.push((*masks).to_owned());
        }
        f.push(format!(
            "scale={w}:{h}:force_original_aspect_ratio=decrease,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps},format=yuv420p"
        ));
        if banners {
            f.push(format!(
                "drawtext=textfile=t{i}.txt:fontcolor=white:fontsize={font}:\
                 box=1:boxcolor=black@0.55:boxborderw=8:x=16:y=16"
            ));
        }
        graph.push_str(&format!("[{i}:v:0]{}[v{i}];", f.join(",")));
        if sound {
            let length = seconds(*length_ms);
            if *audio {
                graph.push_str(&format!(
                    "[{i}:a:0]atrim=start={}:duration={length},asetpts=PTS-STARTPTS,\
                     aresample=48000,aformat=sample_fmts=fltp:channel_layouts=mono,\
                     apad=whole_dur={length}[a{i}];",
                    seconds(*offset_ms)
                ));
            } else {
                graph.push_str(&format!(
                    "anullsrc=r=48000:cl=mono,atrim=duration={length},\
                     aformat=sample_fmts=fltp:channel_layouts=mono[a{i}];"
                ));
            }
        }
    }
    for i in 0..parts.len() {
        graph.push_str(&format!("[v{i}]"));
        if sound {
            graph.push_str(&format!("[a{i}]"));
        }
    }
    if sound {
        graph.push_str(&format!("concat=n={}:v=1:a=1[out][aout]", parts.len()));
    } else {
        graph.push_str(&format!("concat=n={}:v=1:a=0[out]", parts.len()));
    }
    graph
}

/// Local `HH:MM:SS` of a moment, for the banners of a journey video.
pub fn clock(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|t| {
            t.to_zoned_datetime(jiff::tz::TimeZone::system())
                .strftime("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

/// `GET /journeys/settings`: the arrows between cameras.
pub(crate) async fn settings(State(state): State<Arc<ApiState>>) -> Json<JourneySettings> {
    Json(state.journeys())
}

/// `PUT /journeys/settings` (administrators): replaces the arrows.
pub(crate) async fn set_settings(
    State(state): State<Arc<ApiState>>,
    Json(input): Json<JourneySettings>,
) -> Result<Json<JourneySettings>, ApiError> {
    let cameras = state.cameras();
    let ids: Vec<&str> = cameras.iter().map(|c| c.id.as_str()).collect();
    input.validate(&ids).map_err(ApiError::bad_request)?;
    let json = serde_json::to_string(&input).map_err(internal)?;
    database(&state)?
        .set_journey_settings(&json)
        .await
        .map_err(internal)?;
    state.set_journeys(input);
    Ok(Json(state.journeys()))
}

/// A journey (`GET /journeys/{id}`).
#[derive(Clone, Debug, Serialize)]
pub struct JourneyDto {
    /// Identifier: that of the event that started it (usually its first;
    /// an event the AI recognized later may come before it).
    pub id: String,
    /// Kind of object followed (`person`, `car`…); `null` when unknown.
    pub label: Option<String>,
    /// `arrival` or `departure` when every step agrees with the arrows of
    /// now; `null` otherwise.
    pub direction: Option<JourneyDirection>,
    /// Start of its first event.
    pub start_ms: i64,
    /// End of its last event; `null` while one is ongoing.
    pub end_ms: Option<i64>,
    /// Its events in order, one per camera.
    pub events: Vec<EventDto>,
}

/// The kind of object seen on every camera of the journey, or else on its
/// first one.
fn label(events: &[DetectionEvent]) -> Option<String> {
    let first = events.first()?;
    first
        .objects
        .iter()
        .map(|o| &o.label)
        .find(|l| {
            events
                .iter()
                .all(|e| e.objects.iter().any(|o| &o.label == *l))
        })
        .or_else(|| first.objects.first().map(|o| &o.label))
        .cloned()
}

/// The journey made of `events` (in order), as the API shows it.
pub(crate) fn journey_dto(
    id: i64,
    events: &[DetectionEvent],
    settings: &JourneySettings,
) -> Option<JourneyDto> {
    let first = events.first()?;
    let steps: Option<Vec<bool>> = events
        .windows(2)
        .map(|w| {
            settings
                .step(w[0].camera.as_str(), w[1].camera.as_str())
                .map(|s| s.forward)
        })
        .collect();
    let end_ms = if events.iter().all(|e| e.end_ms.is_some()) {
        events.iter().filter_map(|e| e.end_ms).max()
    } else {
        None
    };
    Some(JourneyDto {
        id: id.to_string(),
        label: label(events),
        direction: steps.and_then(|s| direction(&s)),
        start_ms: first.start_ms,
        end_ms,
        events: events.iter().map(EventDto::from).collect(),
    })
}

/// `GET /journeys/{id}`: a journey and its events (those of the cameras
/// the account sees).
pub(crate) async fn journey(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
    Path(id): Path<String>,
) -> Result<Json<JourneyDto>, ApiError> {
    let journey: i64 = id
        .parse()
        .map_err(|_| ApiError::bad_request("invalid journey identifier"))?;
    let events: Vec<DetectionEvent> = database(&state)?
        .journey_events(journey)
        .await
        .map_err(internal)?
        .into_iter()
        .filter(|e| crate::auth::sees(principal.as_ref(), e.camera.as_str()))
        .collect();
    journey_dto(journey, &events, &state.journeys())
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no journey {id}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnvr_core::journey::JourneyLink;
    use rustnvr_core::{CameraId, DetectedObject, EventKind};

    fn event(
        id: i64,
        camera: &str,
        start: i64,
        end: Option<i64>,
        labels: &[&str],
    ) -> DetectionEvent {
        DetectionEvent {
            id,
            camera: CameraId::new(camera).expect("id"),
            kind: EventKind::Motion,
            start_ms: start,
            end_ms: end,
            score: 0.1,
            zones: Vec::new(),
            has_thumbnail: false,
            objects: labels
                .iter()
                .map(|l| DetectedObject {
                    label: (*l).into(),
                    score: 0.9,
                    bbox: None,
                    zone: None,
                    plate: None,
                    plate_owner: None,
                })
                .collect(),
            journey: Some(1),
        }
    }

    #[test]
    fn describes_a_journey() {
        let settings = JourneySettings {
            links: vec![
                JourneyLink {
                    from: "sonnette".into(),
                    to: "entree".into(),
                    max_delay_s: 30,
                    both_ways: true,
                },
                JourneyLink {
                    from: "entree".into(),
                    to: "salon".into(),
                    max_delay_s: 30,
                    both_ways: true,
                },
            ],
        };
        let arrival = [
            event(1, "sonnette", 1_000, Some(5_000), &["car", "person"]),
            event(2, "entree", 9_000, Some(12_000), &["person"]),
            event(3, "salon", 14_000, Some(20_000), &["person"]),
        ];
        let j = journey_dto(1, &arrival, &settings).expect("journey");
        assert_eq!(j.label.as_deref(), Some("person"));
        assert_eq!(j.direction, Some(JourneyDirection::Arrival));
        assert_eq!((j.start_ms, j.end_ms), (1_000, Some(20_000)));
        assert_eq!(j.events.len(), 3);

        let leaving = [
            event(4, "salon", 1_000, Some(2_000), &["person"]),
            event(5, "entree", 3_000, None, &["person"]),
        ];
        let j = journey_dto(4, &leaving, &settings).expect("journey");
        assert_eq!(j.direction, Some(JourneyDirection::Departure));
        assert_eq!(j.end_ms, None, "still ongoing");

        // An arrow removed since: no direction.
        let j = journey_dto(1, &arrival, &JourneySettings::default()).expect("journey");
        assert_eq!(j.direction, None);
        assert!(journey_dto(9, &[], &settings).is_none());
    }

    #[test]
    fn cuts_and_joins_the_cameras_of_a_video() {
        let steps = [
            (10_000, Some(14_000)),
            (18_000, Some(30_000)),
            (26_000, Some(31_000)),
        ];
        // 2 s before; the next camera as soon as it sees it; 2 s after.
        assert_eq!(
            video_periods(&steps, 2_000, 2_000, 20_000, 60_000),
            [(0, 8_000, 18_000), (1, 18_000, 26_000), (2, 26_000, 33_000)]
        );
        // At most per camera, and in all.
        let long: Vec<(i64, Option<i64>)> = (0..5)
            .map(|i| (i * 100_000, Some(i * 100_000 + 90_000)))
            .collect();
        let periods = video_periods(&long, 2_000, 2_000, 20_000, 60_000);
        assert_eq!(periods.len(), 3);
        assert!(periods.iter().all(|(_, from, to)| to - from == 20_000));
        let g = video_graph(
            &[("drawbox=x=0", 1_500, 8_000, false), ("", 0, 2_000, false)],
            (1280, 720),
            25,
            true,
        );
        assert!(
            g.starts_with(
                "[0:v:0]trim=start=1.500:duration=8.000,setpts=PTS-STARTPTS,drawbox=x=0,scale=1280:720"
            ),
            "{g}"
        );
        assert!(g.contains("fps=25") && g.contains("textfile=t1.txt"), "{g}");
        assert!(g.ends_with("[v0][v1]concat=n=2:v=1:a=0[out]"), "{g}");
        assert!(!video_graph(&[("", 0, 1_000, false)], (960, 540), 15, false).contains("drawtext"));
        // Sound on one camera: its sound, then silence as long as the other.
        let g = video_graph(
            &[("", 1_500, 8_000, true), ("", 0, 2_000, false)],
            (1280, 720),
            25,
            false,
        );
        assert!(
            g.contains("[0:a:0]atrim=start=1.500:duration=8.000,asetpts=PTS-STARTPTS"),
            "{g}"
        );
        assert!(g.contains("apad=whole_dur=8.000[a0]"), "{g}");
        assert!(
            g.contains("anullsrc=r=48000:cl=mono,atrim=duration=2.000"),
            "{g}"
        );
        assert!(!g.contains("[1:a:0]"), "{g}");
        assert!(
            g.ends_with("[v0][a0][v1][a1]concat=n=2:v=1:a=1[out][aout]"),
            "{g}"
        );
    }
}
