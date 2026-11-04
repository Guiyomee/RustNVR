//! Recent log messages, kept in memory for the interface (`GET /logs`).
//!
//! They are the messages of the console log (the same filter), which never
//! contain passwords nor full RTSP URLs. The buffer is bounded: the oldest
//! messages are dropped.

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;

/// Severity, most severe first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Failure.
    Error,
    /// Problem that does not stop RustNVR.
    Warn,
    /// Normal operation.
    Info,
    /// Details.
    Debug,
    /// Very detailed.
    Trace,
}

impl LogLevel {
    /// Parses `error`, `warn`, `info`, `debug` or `trace`.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "error" => Self::Error,
            "warn" => Self::Warn,
            "info" => Self::Info,
            "debug" => Self::Debug,
            "trace" => Self::Trace,
            _ => return None,
        })
    }
}

/// One log message.
#[derive(Clone, Debug, Serialize)]
pub struct LogEntry {
    /// Increasing number, for incremental reading (`since`).
    pub seq: u64,
    /// Time, milliseconds since the Unix epoch.
    pub time_ms: i64,
    /// Severity.
    pub level: LogLevel,
    /// Module that wrote it, e.g. `rustnvr_app::nvr`.
    pub target: String,
    /// Camera concerned, if any.
    pub camera: Option<String>,
    /// Message followed by its fields (`key=value`).
    pub message: String,
}

/// Bounded buffer of the latest messages.
pub struct LogBuffer {
    capacity: usize,
    inner: Mutex<(u64, VecDeque<LogEntry>)>,
}

impl LogBuffer {
    /// Keeps the latest `capacity` messages.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            inner: Mutex::new((0, VecDeque::new())),
        }
    }

    /// Adds a message (its `seq` is assigned here).
    pub fn push(&self, mut entry: LogEntry) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.0 += 1;
        entry.seq = inner.0;
        if inner.1.len() >= self.capacity {
            inner.1.pop_front();
        }
        inner.1.push_back(entry);
    }

    /// Messages after `since`, at least as severe as `level`, about `camera`
    /// if given: the latest `limit` of them, oldest first; and the last
    /// `seq` written.
    pub fn query(
        &self,
        since: u64,
        level: LogLevel,
        camera: Option<&str>,
        limit: usize,
    ) -> (Vec<LogEntry>, u64) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<LogEntry> = inner
            .1
            .iter()
            .rev()
            .take_while(|e| e.seq > since)
            .filter(|e| e.level <= level)
            .filter(|e| camera.is_none_or(|c| e.camera.as_deref() == Some(c)))
            .take(limit)
            .cloned()
            .collect();
        out.reverse();
        (out, inner.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(level: LogLevel, camera: Option<&str>, message: &str) -> LogEntry {
        LogEntry {
            seq: 0,
            time_ms: 0,
            level,
            target: "test".into(),
            camera: camera.map(str::to_owned),
            message: message.into(),
        }
    }

    #[test]
    fn keeps_the_latest_and_filters() {
        let logs = LogBuffer::new(3);
        logs.push(entry(LogLevel::Info, None, "a"));
        logs.push(entry(LogLevel::Warn, Some("jardin"), "b"));
        logs.push(entry(LogLevel::Error, Some("entree"), "c"));
        logs.push(entry(LogLevel::Info, Some("jardin"), "d"));
        // Bounded: « a » is gone.
        let (all, last) = logs.query(0, LogLevel::Trace, None, 100);
        assert_eq!(
            all.iter().map(|e| e.message.as_str()).collect::<Vec<_>>(),
            ["b", "c", "d"]
        );
        assert_eq!(last, 4);
        // Warnings and errors only.
        let (warn, _) = logs.query(0, LogLevel::Warn, None, 100);
        assert_eq!(warn.iter().map(|e| e.seq).collect::<Vec<_>>(), [2, 3]);
        // One camera.
        let (jardin, _) = logs.query(0, LogLevel::Trace, Some("jardin"), 100);
        assert_eq!(jardin.len(), 2);
        // Incremental and limited to the latest.
        let (after, _) = logs.query(3, LogLevel::Trace, None, 100);
        assert_eq!(after.iter().map(|e| e.seq).collect::<Vec<_>>(), [4]);
        let (latest, _) = logs.query(0, LogLevel::Trace, None, 1);
        assert_eq!(latest[0].message, "d");
    }
}
