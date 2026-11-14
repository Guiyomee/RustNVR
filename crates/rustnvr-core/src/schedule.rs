//! Weekly schedules: when the motion detection of a camera, or the
//! notifications of a webhook, are active. A schedule is a list of time
//! ranges; without any range, it is always active.

use crate::config::minute_of_day;
use serde::{Deserialize, Serialize};

/// Most ranges in one schedule.
pub const MAX_RANGES: usize = 16;

/// Some days of the week, from one time to another (local time of the
/// server). When `to` is earlier than `from`, the range goes on until `to`
/// the next day (`22:00`–`06:00`: a night); when they are equal, the whole
/// day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeRange {
    /// Days the range starts on, 1 (Monday) to 7 (Sunday); empty: every day.
    #[serde(default)]
    pub days: Vec<u8>,
    /// Start, `HH:MM`.
    pub from: String,
    /// End, `HH:MM`.
    pub to: String,
}

impl TimeRange {
    fn starts_on(&self, day: u8) -> bool {
        self.days.is_empty() || self.days.contains(&day)
    }

    /// Whether minute `minute` (0..1440) of weekday `day` (1 = Monday) is in
    /// the range.
    pub fn contains(&self, day: u8, minute: u16) -> bool {
        let (Some(from), Some(to)) = (minute_of_day(&self.from), minute_of_day(&self.to)) else {
            return false;
        };
        let yesterday = if day <= 1 { 7 } else { day - 1 };
        if from == to {
            self.starts_on(day)
        } else if from < to {
            self.starts_on(day) && (from..to).contains(&minute)
        } else {
            (self.starts_on(day) && minute >= from) || (self.starts_on(yesterday) && minute < to)
        }
    }
}

/// Whether a schedule is active at minute `minute` of weekday `day`
/// (1 = Monday): always without ranges, else within one of them.
pub fn active(schedule: &[TimeRange], day: u8, minute: u16) -> bool {
    schedule.is_empty() || schedule.iter().any(|r| r.contains(day, minute))
}

/// Every problem of a schedule; `what` names it in the messages.
pub fn problems(schedule: &[TimeRange], what: &str) -> Vec<String> {
    let mut p = Vec::new();
    if schedule.len() > MAX_RANGES {
        p.push(format!("{what} : {MAX_RANGES} plages au plus"));
    }
    for r in schedule {
        if minute_of_day(&r.from).is_none() || minute_of_day(&r.to).is_none() {
            p.push(format!(
                "{what} : heures attendues comme 08:30 (reçu {:?} – {:?})",
                r.from, r.to
            ));
        }
        if r.days.iter().any(|d| !(1..=7).contains(d)) {
            p.push(format!("{what} : jours de 1 (lundi) à 7 (dimanche)"));
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(days: &[u8], from: &str, to: &str) -> TimeRange {
        TimeRange {
            days: days.to_vec(),
            from: from.into(),
            to: to.into(),
        }
    }

    const fn at(h: u16, m: u16) -> u16 {
        h * 60 + m
    }

    #[test]
    fn ranges_by_day_and_across_midnight() {
        // Working days, 08:00 to 18:00.
        let office = range(&[1, 2, 3, 4, 5], "08:00", "18:00");
        assert!(office.contains(1, at(8, 0)) && office.contains(5, at(17, 59)));
        assert!(!office.contains(1, at(18, 0)) && !office.contains(6, at(10, 0)));
        // Every night, 22:00 to 06:00.
        let night = range(&[], "22:00", "06:00");
        assert!(night.contains(3, at(23, 0)) && night.contains(4, at(5, 59)));
        assert!(!night.contains(4, at(6, 0)) && !night.contains(4, at(12, 0)));
        // Friday and Saturday nights: until Saturday and Sunday mornings.
        let weekend = range(&[5, 6], "23:00", "07:00");
        assert!(weekend.contains(6, at(3, 0)) && weekend.contains(7, at(3, 0)));
        assert!(!weekend.contains(5, at(3, 0)), "Thursday night");
        // Sunday night goes on into Monday.
        let sunday = range(&[7], "20:00", "02:00");
        assert!(sunday.contains(1, at(1, 0)) && !sunday.contains(7, at(1, 0)));
        // Equal times: the whole day.
        let saturday = range(&[6], "00:00", "00:00");
        assert!(saturday.contains(6, at(0, 0)) && saturday.contains(6, at(23, 59)));
        assert!(!saturday.contains(7, at(0, 0)));
    }

    #[test]
    fn a_schedule_without_ranges_is_always_active() {
        assert!(active(&[], 3, at(12, 0)));
        let s = [
            range(&[6, 7], "00:00", "00:00"),
            range(&[], "22:00", "06:00"),
        ];
        assert!(active(&s, 6, at(12, 0)) && active(&s, 2, at(23, 0)));
        assert!(!active(&s, 2, at(12, 0)));
        assert!(problems(&s, "x").is_empty());
        let bad = [range(&[0, 8], "25:00", "6h")];
        assert_eq!(problems(&bad, "Détection").len(), 2);
        assert!(problems(&vec![range(&[], "01:00", "02:00"); 17], "x")[0].contains("16"));
    }
}
