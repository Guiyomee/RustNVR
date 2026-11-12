//! Activity statistics (`GET /events/stats?days=7|30`): detection events
//! counted by day, by hour of the day, by camera and by recognized object, in
//! the local time of the server.

use crate::routes::ApiError;
use crate::state::{ApiState, unix_ms};
use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

const DAY_MS: i64 = 86_400_000;

/// Query of `GET /events/stats`.
#[derive(Deserialize)]
pub(crate) struct StatsQuery {
    /// 7 (default) or 30.
    days: Option<u32>,
}

/// One count.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub(crate) struct Count {
    key: String,
    count: u64,
}

/// Body of `GET /events/stats`.
#[derive(Serialize, Debug)]
pub(crate) struct StatsDto {
    from_ms: i64,
    to_ms: i64,
    days: u32,
    total: u64,
    /// Every day of the period (`YYYY-MM-DD`, local time), oldest first.
    by_day: Vec<Count>,
    /// Hours 0 to 23 (local time).
    by_hour: Vec<u64>,
    /// Cameras, most active first.
    by_camera: Vec<Count>,
    /// Recognized objects (labels), most frequent first; an event counts
    /// once per kind of object.
    by_object: Vec<Count>,
    /// Events without recognized object.
    without_object: u64,
}

fn sorted(map: BTreeMap<String, u64>) -> Vec<Count> {
    let mut v: Vec<Count> = map
        .into_iter()
        .map(|(key, count)| Count { key, count })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
    v
}

/// Counts `events` (camera, start, labels) over `days` days ending at `now_ms`.
fn compute(
    events: &[(String, i64, Vec<String>)],
    days: u32,
    now_ms: i64,
    tz: &jiff::tz::TimeZone,
) -> StatsDto {
    let local = |ms: i64| {
        jiff::Timestamp::from_millisecond(ms)
            .ok()
            .map(|t| t.to_zoned(tz.clone()))
    };
    let from_ms = now_ms - i64::from(days) * DAY_MS;
    // Every day of the period, even without events.
    let mut by_day: BTreeMap<String, u64> = BTreeMap::new();
    let mut day = from_ms;
    while day <= now_ms {
        if let Some(z) = local(day) {
            by_day.insert(z.date().to_string(), 0);
        }
        day += DAY_MS;
    }
    if let Some(z) = local(now_ms) {
        by_day.insert(z.date().to_string(), 0);
    }
    let mut by_hour = vec![0u64; 24];
    let mut by_camera: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_object: BTreeMap<String, u64> = BTreeMap::new();
    let mut without_object = 0;
    let mut total = 0;
    for (camera, start_ms, labels) in events {
        if *start_ms < from_ms || *start_ms > now_ms {
            continue;
        }
        let Some(z) = local(*start_ms) else { continue };
        total += 1;
        *by_day.entry(z.date().to_string()).or_default() += 1;
        by_hour[usize::try_from(z.hour()).unwrap_or(0).min(23)] += 1;
        *by_camera.entry(camera.clone()).or_default() += 1;
        if labels.is_empty() {
            without_object += 1;
        }
        for l in labels {
            *by_object.entry(l.clone()).or_default() += 1;
        }
    }
    StatsDto {
        from_ms,
        to_ms: now_ms,
        days,
        total,
        by_day: by_day
            .into_iter()
            .map(|(key, count)| Count { key, count })
            .collect(),
        by_hour,
        by_camera: sorted(by_camera),
        by_object: sorted(by_object),
        without_object,
    }
}

/// `GET /events/stats?days=7|30`.
pub(crate) async fn stats(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
    Query(q): Query<StatsQuery>,
) -> Result<Json<StatsDto>, ApiError> {
    let days = q.days.unwrap_or(7);
    if days != 7 && days != 30 {
        return Err(ApiError::bad_request("days : 7 ou 30"));
    }
    let db = state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("base de données indisponible : pas d'événements"))?;
    let now = unix_ms(std::time::SystemTime::now());
    let mut events = db
        .event_summaries(now - i64::from(days) * DAY_MS, now + 1)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    events.retain(|(camera, ..)| crate::auth::sees(principal.as_ref(), camera));
    Ok(Json(compute(
        &events,
        days,
        now,
        &jiff::tz::TimeZone::system(),
    )))
}

/// Crossings of one line.
#[derive(Serialize, Debug)]
pub(crate) struct LineCounts {
    camera: String,
    line: String,
    total: u64,
    /// From the left of the line (as drawn) to its right.
    forward: u64,
    backward: u64,
    /// Since local midnight.
    today: u64,
    /// Every day of the period (`YYYY-MM-DD`, local time), oldest first.
    by_day: Vec<Count>,
    /// Who is inside, for a line counting the presence.
    present: Option<u64>,
}

/// Body of `GET /crossings`.
#[derive(Serialize, Debug)]
pub(crate) struct CrossingsDto {
    from_ms: i64,
    to_ms: i64,
    days: u32,
    lines: Vec<LineCounts>,
}

/// Forward and backward crossings of a line, and its crossings per day.
type PerLine = (u64, u64, BTreeMap<String, u64>);

/// Counts `crossings` (camera, line, forward, time) per line.
fn count_crossings(
    crossings: &[(String, String, bool, i64)],
    presence: &[(String, String, u64)],
    days: u32,
    now_ms: i64,
    tz: &jiff::tz::TimeZone,
) -> CrossingsDto {
    let local = |ms: i64| {
        jiff::Timestamp::from_millisecond(ms)
            .ok()
            .map(|t| t.to_zoned(tz.clone()))
    };
    let from_ms = now_ms - i64::from(days) * DAY_MS;
    let mut empty_days: BTreeMap<String, u64> = BTreeMap::new();
    let mut day = from_ms;
    while day <= now_ms {
        if let Some(z) = local(day) {
            empty_days.insert(z.date().to_string(), 0);
        }
        day += DAY_MS;
    }
    let today = local(now_ms)
        .map(|z| z.date().to_string())
        .unwrap_or_default();
    empty_days.insert(today.clone(), 0);
    let mut lines: BTreeMap<(String, String), PerLine> = BTreeMap::new();
    // Lines counting the presence appear even without crossings in the period.
    for (camera, line, _) in presence {
        lines
            .entry((camera.clone(), line.clone()))
            .or_insert_with(|| (0, 0, empty_days.clone()));
    }
    for (camera, line, forward, at_ms) in crossings {
        if *at_ms < from_ms || *at_ms > now_ms {
            continue;
        }
        let Some(z) = local(*at_ms) else { continue };
        let entry = lines
            .entry((camera.clone(), line.clone()))
            .or_insert_with(|| (0, 0, empty_days.clone()));
        if *forward {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
        *entry.2.entry(z.date().to_string()).or_default() += 1;
    }
    CrossingsDto {
        from_ms,
        to_ms: now_ms,
        days,
        lines: lines
            .into_iter()
            .map(|((camera, line), (forward, backward, by_day))| LineCounts {
                present: presence
                    .iter()
                    .find(|(c, l, _)| *c == camera && *l == line)
                    .map(|(_, _, n)| *n),
                today: by_day.get(&today).copied().unwrap_or(0),
                camera,
                line,
                total: forward + backward,
                forward,
                backward,
                by_day: by_day
                    .into_iter()
                    .map(|(key, count)| Count { key, count })
                    .collect(),
            })
            .collect(),
    }
}

/// `GET /crossings?days=7|30`: crossings of the lines drawn on the cameras.
pub(crate) async fn crossings(
    State(state): State<Arc<ApiState>>,
    principal: Option<axum::Extension<crate::auth::Principal>>,
    Query(q): Query<StatsQuery>,
) -> Result<Json<CrossingsDto>, ApiError> {
    let days = q.days.unwrap_or(7);
    if days != 7 && days != 30 {
        return Err(ApiError::bad_request("days : 7 ou 30"));
    }
    let db = state
        .database
        .get()
        .ok_or_else(|| ApiError::unavailable("base de données indisponible"))?;
    let now = unix_ms(std::time::SystemTime::now());
    let mut list = db
        .crossings(now - i64::from(days) * DAY_MS, now + 1)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut presence = match state.control.get() {
        Some(c) => c.presence().await,
        None => Vec::new(),
    };
    list.retain(|(camera, ..)| crate::auth::sees(principal.as_ref(), camera));
    presence.retain(|(camera, ..)| crate::auth::sees(principal.as_ref(), camera));
    Ok(Json(count_crossings(
        &list,
        &presence,
        days,
        now,
        &jiff::tz::TimeZone::system(),
    )))
}

/// `POST /cameras/{id}/presence/reset` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PresenceReset {
    line: String,
}

/// `POST /cameras/{id}/presence/reset`: the presence count of a line starts
/// again from zero.
pub(crate) async fn reset_presence(
    State(state): State<Arc<ApiState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(input): Json<PresenceReset>,
) -> Result<axum::http::StatusCode, ApiError> {
    crate::routes::control(&state)?
        .reset_presence(id, input.line)
        .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    #[test]
    fn counts_crossings_per_line() {
        let tz = jiff::tz::TimeZone::UTC;
        let now = 1_793_707_200_000; // 2026-11-03 12:00 UTC
        let c = |line: &str, forward, ago: i64| {
            ("jardin".to_owned(), line.to_owned(), forward, now - ago)
        };
        let list = vec![
            c("portail", true, 60_000),
            c("portail", false, 120_000),
            c("portail", true, 2 * DAY_MS),
            c("allée", true, 1_000),
        ];
        // « porte » counts the presence and was not crossed in the period.
        let presence = vec![
            ("jardin".to_owned(), "portail".to_owned(), 1),
            ("jardin".to_owned(), "porte".to_owned(), 0),
        ];
        let d = count_crossings(&list, &presence, 7, now, &tz);
        assert_eq!(d.lines.len(), 3);
        let porte = d.lines.iter().find(|l| l.line == "porte").expect("porte");
        assert_eq!((porte.total, porte.present), (0, Some(0)));
        let allee = d.lines.iter().find(|l| l.line == "allée").expect("allée");
        assert_eq!(allee.present, None);
        let portail = d.lines.iter().find(|l| l.line == "portail").expect("line");
        assert_eq!(
            (
                portail.total,
                portail.forward,
                portail.backward,
                portail.today
            ),
            (3, 2, 1, 2)
        );
        assert_eq!(portail.present, Some(1));
        assert_eq!(portail.by_day.len(), 8);
    }

    use super::*;

    #[test]
    fn counts_by_day_hour_camera_and_object() {
        let tz = jiff::tz::TimeZone::UTC;
        // 2026-11-03 12:00 UTC.
        let now = 1_793_707_200_000;
        let events = vec![
            (
                "jardin".to_owned(),
                now - 3_600_000,
                vec!["person".to_owned()],
            ),
            ("jardin".to_owned(), now - 2 * DAY_MS, vec![]),
            (
                "entree".to_owned(),
                now - 60_000,
                vec!["car".to_owned(), "person".to_owned()],
            ),
            // Outside the 7 days.
            ("entree".to_owned(), now - 8 * DAY_MS, vec![]),
        ];
        let s = compute(&events, 7, now, &tz);
        assert_eq!(s.total, 3);
        assert_eq!(s.by_day.len(), 8);
        assert_eq!(
            s.by_day.last().map(|d| (d.key.as_str(), d.count)),
            Some(("2026-11-03", 2))
        );
        assert_eq!(s.by_hour[11], 2);
        assert_eq!(s.by_hour[12], 1);
        assert_eq!(
            s.by_camera[0],
            Count {
                key: "jardin".into(),
                count: 2
            }
        );
        assert_eq!(
            s.by_object[0],
            Count {
                key: "person".into(),
                count: 2
            }
        );
        assert_eq!(s.without_object, 1);
    }
}
