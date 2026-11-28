//! RustNVR database (SQLite, WAL): detection events and per-camera motion
//! detection settings, plus the event thumbnails stored next to it.
//!
//! Recordings are not indexed here (yet): they are found by scanning the
//! recording directories.
//!
//! Layout of the data directory:
//! - `rustnvr.db` (+ `-wal`, `-shm`): the database;
//! - `thumbnails/<camera>/<event id>.jpg`: event thumbnails.

use rustnvr_core::{CameraId, DetectedObject, DetectionEvent, EventKind, MotionSettings};
use sqlx::Row as _;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow,
    SqliteSynchronous,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Database errors.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// SQLite error.
    #[error("database: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// Schema migration error.
    #[error("database migration: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// File system error (data directory, thumbnails).
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// Stored JSON that cannot be read back.
    #[error("database: invalid stored value: {0}")]
    Json(#[from] serde_json::Error),
}

/// Most events returned by one query.
pub const MAX_EVENTS_PER_QUERY: u32 = 1000;

/// Filter of [`Database::events`].
#[derive(Clone, Debug, Default)]
pub struct EventFilter {
    /// Only this camera.
    pub camera: Option<CameraId>,
    /// Only these cameras (`None`: all of them), for an account that does
    /// not see every camera.
    pub cameras: Option<Vec<String>>,
    /// Events overlapping `[from_ms, to_ms)`.
    pub from_ms: i64,
    /// See `from_ms`.
    pub to_ms: i64,
    /// Most events returned (newest first), capped at [`MAX_EVENTS_PER_QUERY`].
    pub limit: u32,
    /// Only events where one of these objects was recognized (labels).
    pub objects: Option<Vec<String>>,
    /// Only events in one of these zones (motion zones, or the zone of a
    /// recognized object).
    pub zones: Option<Vec<String>>,
    /// Only events with a plate read containing this text (capital letters
    /// and digits only: anything else never matches).
    pub plate: Option<String>,
}

/// What [`Database::delete_event`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDeletion {
    /// Deleted, with its thumbnail.
    Deleted,
    /// No such event.
    Missing,
    /// Still going on: kept.
    Ongoing,
}

/// A finished recording segment in the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentRecord {
    /// Storage destination (`local`, …).
    pub destination: String,
    /// Camera identifier.
    pub camera: String,
    /// `YYYY-MM-DD` directory.
    pub day: String,
    /// File name.
    pub file: String,
    /// First frame (ms since the Unix epoch).
    pub start_ms: i64,
    /// End of the last frame.
    pub end_ms: i64,
    /// File size.
    pub bytes: u64,
}

fn segment_from_row(r: &SqliteRow) -> Result<SegmentRecord, DbError> {
    let bytes: i64 = r.try_get("bytes")?;
    Ok(SegmentRecord {
        destination: r.try_get("destination")?,
        camera: r.try_get("camera")?,
        day: r.try_get("day")?,
        file: r.try_get("file")?,
        start_ms: r.try_get("start_ms")?,
        end_ms: r.try_get("end_ms")?,
        bytes: u64::try_from(bytes).unwrap_or(0),
    })
}

/// A bookmark on the timeline: a moment of a camera with a note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    /// Identifier.
    pub id: i64,
    /// Camera identifier.
    pub camera: String,
    /// The moment (ms since the Unix epoch).
    pub at_ms: i64,
    /// The note typed by the user.
    pub note: String,
    /// Account that added it, if any.
    pub author: Option<String>,
    /// When it was added.
    pub created_ms: i64,
}

impl Bookmark {
    fn from_row(r: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: r.try_get("id")?,
            camera: r.try_get("camera")?,
            at_ms: r.try_get("at_ms")?,
            note: r.try_get("note")?,
            author: r.try_get("author")?,
            created_ms: r.try_get("created_ms")?,
        })
    }
}

/// A period of recordings kept whatever the retention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectedRange {
    /// Identifier.
    pub id: i64,
    /// Camera identifier.
    pub camera: String,
    /// Start (ms since the Unix epoch).
    pub start_ms: i64,
    /// End (ms since the Unix epoch).
    pub end_ms: i64,
    /// Why it is kept, as typed by the user.
    pub note: String,
    /// When it was protected.
    pub created_ms: i64,
}

fn protected_from_row(r: &SqliteRow) -> Result<ProtectedRange, DbError> {
    Ok(ProtectedRange {
        id: r.try_get("id")?,
        camera: r.try_get("camera")?,
        start_ms: r.try_get("start_ms")?,
        end_ms: r.try_get("end_ms")?,
        note: r.try_get("note")?,
        created_ms: r.try_get("created_ms")?,
    })
}

/// An account (never its password).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserRecord {
    /// Login name.
    pub username: String,
    /// `admin` or `viewer`.
    pub role: String,
    /// Creation time (ms since the Unix epoch).
    pub created_ms: i64,
    /// Cameras a viewer may see (`None`: all of them).
    pub cameras: Option<Vec<String>>,
    /// The two-step login (TOTP) is on.
    pub totp: bool,
}

/// An API token of an account (its secret is never kept).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiTokenRecord {
    /// Identifier.
    pub id: i64,
    /// Account the token signs in as.
    pub username: String,
    /// Name given at its creation (the device or script).
    pub name: String,
    /// Reading only, even for an administrator.
    pub read_only: bool,
    /// Creation time (ms since the Unix epoch).
    pub created_ms: i64,
    /// Last use, if any.
    pub last_used_ms: Option<i64>,
}

/// Who an API token signs in as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenUser {
    /// Token identifier.
    pub id: i64,
    /// Account.
    pub username: String,
    /// Role of the account.
    pub role: String,
    /// Cameras the account sees (`None`: all of them).
    pub cameras: Option<Vec<String>>,
    /// Reading only.
    pub read_only: bool,
    /// Last use, if any.
    pub last_used_ms: Option<i64>,
}

/// A line of the activity log of the accounts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditEntry {
    /// Identifier, growing with time.
    pub id: i64,
    /// When (ms since the Unix epoch).
    pub at_ms: i64,
    /// Account, when one is known.
    pub username: Option<String>,
    /// Address of the client.
    pub client: String,
    /// `login`, `login_failed`, `logout`, or `METHOD /path` of a request.
    pub action: String,
    /// HTTP status of the answer.
    pub status: u16,
}

/// The `cameras` column of an account: a JSON array, or NULL for all.
fn cameras_from_row(row: &SqliteRow) -> Result<Option<Vec<String>>, DbError> {
    let text: Option<String> = row.try_get("cameras")?;
    Ok(text.map(|t| serde_json::from_str(&t)).transpose()?)
}

/// The database and the thumbnails directory.
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
    thumbnails: PathBuf,
}

fn event_from_row(row: &SqliteRow) -> Result<DetectionEvent, DbError> {
    let camera: String = row.try_get("camera")?;
    let zones: String = row.try_get("zones")?;
    let thumbnail: Option<String> = row.try_get("thumbnail")?;
    let score: f64 = row.try_get("score")?;
    let objects: String = row.try_get("objects")?;
    let kind: String = row.try_get("kind")?;
    Ok(DetectionEvent {
        id: row.try_get("id")?,
        // Identifiers were validated when the event was written.
        camera: CameraId::new(camera).map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        kind: EventKind::from_name(&kind),
        start_ms: row.try_get("start_ms")?,
        end_ms: row.try_get("end_ms")?,
        score: score as f32,
        zones: serde_json::from_str(&zones)?,
        has_thumbnail: thumbnail.is_some(),
        objects: serde_json::from_str(&objects)?,
        journey: row.try_get("journey")?,
    })
}

impl Database {
    /// Opens (creating it if needed) the database in `dir`, applies pending
    /// migrations and closes events left open by a crash at their start.
    pub async fn open(dir: &Path) -> Result<Self, DbError> {
        Self::open_after(dir, None).await
    }

    /// [`Self::open`], closing the events left open by a crash at
    /// `last_alive_ms`, the last moment RustNVR was known to run (instead of
    /// their start), when it is after their start.
    pub async fn open_after(dir: &Path, last_alive_ms: Option<i64>) -> Result<Self, DbError> {
        std::fs::create_dir_all(dir)?;
        let options = SqliteConnectOptions::new()
            .filename(dir.join("rustnvr.db"))
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            // WAL + NORMAL: durable except for the last transactions on power loss.
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));
        // SQLite serializes writes; a few connections are enough for reads.
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let db = Self {
            pool,
            thumbnails: dir.join("thumbnails"),
        };
        let closed = sqlx::query(
            "UPDATE events SET end_ms = \
             CASE WHEN ?1 IS NULL OR ?1 < start_ms THEN start_ms ELSE ?1 END \
             WHERE end_ms IS NULL",
        )
        .bind(last_alive_ms)
        .execute(&db.pool)
        .await?
        .rows_affected();
        if closed > 0 {
            tracing::info!(
                events = closed,
                "closed events left open by an interruption"
            );
        }
        Ok(db)
    }

    /// Records the start of an event and returns it with its identifier.
    pub async fn start_event(
        &self,
        camera: &CameraId,
        kind: EventKind,
        start_ms: i64,
        score: f32,
        zones: &[String],
    ) -> Result<DetectionEvent, DbError> {
        let id = sqlx::query(
            "INSERT INTO events (camera, kind, start_ms, score, zones) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(camera.as_str())
        .bind(kind.as_str())
        .bind(start_ms)
        .bind(f64::from(score))
        .bind(serde_json::to_string(zones)?)
        .execute(&self.pool)
        .await?
        .last_insert_rowid();
        Ok(DetectionEvent {
            id,
            camera: camera.clone(),
            kind,
            start_ms,
            end_ms: None,
            score,
            zones: zones.to_vec(),
            has_thumbnail: false,
            objects: Vec::new(),
            journey: None,
        })
    }

    /// Stores the objects recognized in the thumbnail of event `id`.
    pub async fn set_objects(&self, id: i64, objects: &[DetectedObject]) -> Result<(), DbError> {
        sqlx::query("UPDATE events SET objects = ? WHERE id = ?")
            .bind(serde_json::to_string(objects)?)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Updates the end (or `None` while ongoing), score and zones of an event.
    pub async fn update_event(&self, event: &DetectionEvent) -> Result<(), DbError> {
        sqlx::query("UPDATE events SET end_ms = ?, score = ?, zones = ? WHERE id = ?")
            .bind(event.end_ms)
            .bind(f64::from(event.score))
            .bind(serde_json::to_string(&event.zones)?)
            .bind(event.id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Stores the thumbnail of an event (JPEG).
    pub async fn set_thumbnail(&self, event: &DetectionEvent, jpeg: &[u8]) -> Result<(), DbError> {
        let dir = self.thumbnails.join(event.camera.as_str());
        let name = format!("{}.jpg", event.id);
        let jpeg = jpeg.to_vec();
        let path = dir.join(&name);
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(path, jpeg)
        })
        .await
        .map_err(std::io::Error::other)??;
        sqlx::query("UPDATE events SET thumbnail = ? WHERE id = ?")
            .bind(format!("{}/{name}", event.camera.as_str()))
            .bind(event.id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// One event.
    pub async fn event(&self, id: i64) -> Result<Option<DetectionEvent>, DbError> {
        sqlx::query("SELECT * FROM events WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| event_from_row(&r))
            .transpose()
    }

    /// Path of the thumbnail of an event, if it has one.
    pub async fn thumbnail_path(&self, id: i64) -> Result<Option<PathBuf>, DbError> {
        let name: Option<Option<String>> =
            sqlx::query_scalar("SELECT thumbnail FROM events WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(name.flatten().map(|n| self.thumbnails.join(n)))
    }

    /// Events overlapping the filter's period, newest first.
    pub async fn events(&self, f: &EventFilter) -> Result<Vec<DetectionEvent>, DbError> {
        let limit = f.limit.clamp(1, MAX_EVENTS_PER_QUERY);
        let cameras = f.cameras.as_ref().map(serde_json::to_string).transpose()?;
        let objects = f.objects.as_ref().map(serde_json::to_string).transpose()?;
        let zones = f.zones.as_ref().map(serde_json::to_string).transpose()?;
        // Letters and digits only, so no LIKE wildcard gets in.
        let plate = f
            .plate
            .as_deref()
            .filter(|p| {
                p.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            })
            .map(|p| format!("%{p}%"));
        if f.plate.is_some() && plate.is_none() {
            return Ok(Vec::new());
        }
        // An ongoing event (end_ms NULL) overlaps any period after its start.
        let rows = sqlx::query(
            "SELECT * FROM events \
             WHERE (?1 IS NULL OR camera = ?1) AND start_ms < ?3 AND COALESCE(end_ms, ?3) >= ?2 \
             AND (?5 IS NULL OR camera IN (SELECT value FROM json_each(?5))) \
             AND (?6 IS NULL OR EXISTS (SELECT 1 FROM json_each(events.objects) o \
                  WHERE json_extract(o.value, '$.label') IN (SELECT value FROM json_each(?6)))) \
             AND (?7 IS NULL OR EXISTS (SELECT 1 FROM json_each(events.zones) z \
                  WHERE z.value IN (SELECT value FROM json_each(?7))) \
                  OR EXISTS (SELECT 1 FROM json_each(events.objects) o \
                  WHERE json_extract(o.value, '$.zone') IN (SELECT value FROM json_each(?7)))) \
             AND (?8 IS NULL OR EXISTS (SELECT 1 FROM json_each(events.objects) o \
                  WHERE json_extract(o.value, '$.plate') LIKE ?8)) \
             ORDER BY start_ms DESC LIMIT ?4",
        )
        .bind(f.camera.as_ref().map(CameraId::as_str))
        .bind(f.from_ms)
        .bind(f.to_ms)
        .bind(i64::from(limit))
        .bind(cameras)
        .bind(objects)
        .bind(zones)
        .bind(plate)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(event_from_row).collect()
    }

    /// Deletes the events that started before `before_ms`, with their
    /// thumbnails; returns how many were deleted.
    pub async fn delete_events_before(&self, before_ms: i64) -> Result<u64, DbError> {
        let thumbnails: Vec<String> = sqlx::query_scalar(
            "SELECT thumbnail FROM events \
             WHERE start_ms < ? AND end_ms IS NOT NULL AND thumbnail IS NOT NULL",
        )
        .bind(before_ms)
        .fetch_all(&self.pool)
        .await?;
        let deleted = sqlx::query("DELETE FROM events WHERE start_ms < ? AND end_ms IS NOT NULL")
            .bind(before_ms)
            .execute(&self.pool)
            .await?
            .rows_affected();
        let root = self.thumbnails.clone();
        tokio::task::spawn_blocking(move || {
            for t in thumbnails {
                let _ = std::fs::remove_file(root.join(t));
            }
        })
        .await
        .map_err(std::io::Error::other)?;
        Ok(deleted)
    }

    /// Deletes one finished event with its thumbnail. The other events of
    /// its journey stay together: the journey is then named after the first
    /// one left, and is no journey any more with a single event.
    pub async fn delete_event(&self, id: i64) -> Result<EventDeletion, DbError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT end_ms, thumbnail, journey FROM events WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            return Ok(EventDeletion::Missing);
        };
        let end: Option<i64> = row.try_get("end_ms")?;
        if end.is_none() {
            return Ok(EventDeletion::Ongoing);
        }
        let thumbnail: Option<String> = row.try_get("thumbnail")?;
        let journey: Option<i64> = row.try_get("journey")?;
        sqlx::query("DELETE FROM events WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if let Some(journey) = journey {
            let left: Vec<i64> =
                sqlx::query_scalar("SELECT id FROM events WHERE journey = ? ORDER BY start_ms, id")
                    .bind(journey)
                    .fetch_all(&mut *tx)
                    .await?;
            let named = (left.len() > 1).then(|| left[0]);
            if named != Some(journey) {
                sqlx::query("UPDATE events SET journey = ? WHERE journey = ?")
                    .bind(named)
                    .bind(journey)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        if let Some(t) = thumbnail {
            let path = self.thumbnails.join(t);
            let _ = tokio::fs::remove_file(path).await;
        }
        Ok(EventDeletion::Deleted)
    }

    /// Motion detection settings of every camera that has some.
    pub async fn motion_settings(&self) -> Result<HashMap<String, MotionSettings>, DbError> {
        let rows = sqlx::query("SELECT camera, settings FROM motion_settings")
            .fetch_all(&self.pool)
            .await?;
        let mut out = HashMap::new();
        for r in rows {
            let settings: String = r.try_get("settings")?;
            out.insert(r.try_get("camera")?, serde_json::from_str(&settings)?);
        }
        Ok(out)
    }

    /// Stores the motion detection settings of a camera.
    pub async fn set_motion_settings(
        &self,
        camera: &CameraId,
        settings: &MotionSettings,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO motion_settings (camera, settings) VALUES (?, ?) \
             ON CONFLICT (camera) DO UPDATE SET settings = excluded.settings",
        )
        .bind(camera.as_str())
        .bind(serde_json::to_string(settings)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Forgets the motion detection settings of a removed camera.
    pub async fn delete_motion_settings(&self, camera: &CameraId) -> Result<(), DbError> {
        sqlx::query("DELETE FROM motion_settings WHERE camera = ?")
            .bind(camera.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Every account, by name.
    pub async fn users(&self) -> Result<Vec<UserRecord>, DbError> {
        let rows =
            sqlx::query("SELECT username, role, created_ms, cameras, totp_secret IS NOT NULL AS totp FROM users ORDER BY username")
                .fetch_all(&self.pool)
                .await?;
        rows.iter()
            .map(|r| {
                Ok(UserRecord {
                    username: r.try_get("username")?,
                    role: r.try_get("role")?,
                    created_ms: r.try_get("created_ms")?,
                    cameras: cameras_from_row(r)?,
                    totp: r.try_get("totp")?,
                })
            })
            .collect()
    }

    /// Password hash and role of an account.
    pub async fn user_secret(&self, username: &str) -> Result<Option<(String, String)>, DbError> {
        let row = sqlx::query("SELECT password_hash, role FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| Ok((r.try_get("password_hash")?, r.try_get("role")?)))
            .transpose()
    }

    /// Cameras an account may see (`None`: all of them, also for an unknown
    /// account).
    pub async fn user_cameras(&self, username: &str) -> Result<Option<Vec<String>>, DbError> {
        let row = sqlx::query("SELECT cameras FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(&self.pool)
            .await?;
        row.map_or(Ok(None), |r| cameras_from_row(&r))
    }

    /// Sets the cameras an account may see (`None`: all of them); false if
    /// the account does not exist.
    pub async fn set_user_cameras(
        &self,
        username: &str,
        cameras: Option<&[String]>,
    ) -> Result<bool, DbError> {
        let json = cameras.map(serde_json::to_string).transpose()?;
        let done = sqlx::query("UPDATE users SET cameras = ? WHERE username = ?")
            .bind(json)
            .bind(username)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Creates an account; false if the name is taken.
    pub async fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        role: &str,
        now_ms: i64,
    ) -> Result<bool, DbError> {
        let done = sqlx::query(
            "INSERT INTO users (username, password_hash, role, created_ms) VALUES (?, ?, ?, ?) \n             ON CONFLICT (username) DO NOTHING",
        )
        .bind(username)
        .bind(password_hash)
        .bind(role)
        .bind(now_ms)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Changes the password and/or role of an account; false if it does not exist.
    pub async fn update_user(
        &self,
        username: &str,
        password_hash: Option<&str>,
        role: Option<&str>,
    ) -> Result<bool, DbError> {
        let done = sqlx::query(
            "UPDATE users SET password_hash = COALESCE(?, password_hash), role = COALESCE(?, role) \n             WHERE username = ?",
        )
        .bind(password_hash)
        .bind(role)
        .bind(username)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Deletes an account, its sessions, its API tokens and its recovery
    /// codes; false if it did not exist.
    pub async fn delete_user(&self, username: &str) -> Result<bool, DbError> {
        self.delete_sessions_of(username).await?;
        sqlx::query("DELETE FROM api_tokens WHERE username = ?")
            .bind(username)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM recovery_codes WHERE username = ?")
            .bind(username)
            .execute(&self.pool)
            .await?;
        let done = sqlx::query("DELETE FROM users WHERE username = ?")
            .bind(username)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() == 1)
    }

    /// The TOTP secret of an account (base32), if its two-step login is on.
    pub async fn user_totp(&self, username: &str) -> Result<Option<String>, DbError> {
        Ok(
            sqlx::query("SELECT totp_secret FROM users WHERE username = ?")
                .bind(username)
                .fetch_optional(&self.pool)
                .await?
                .map(|r| r.try_get::<Option<String>, _>("totp_secret"))
                .transpose()?
                .flatten(),
        )
    }

    /// Turns the two-step login of an account on with `secret`, with these
    /// recovery codes (SHA-256), or off (`None`, codes forgotten); false if
    /// the account does not exist.
    pub async fn set_user_totp(
        &self,
        username: &str,
        secret: Option<&str>,
        recovery_hashes: &[String],
    ) -> Result<bool, DbError> {
        let mut tx = self.pool.begin().await?;
        let done =
            sqlx::query("UPDATE users SET totp_secret = ?, totp_step = NULL WHERE username = ?")
                .bind(secret)
                .bind(username)
                .execute(&mut *tx)
                .await?;
        sqlx::query("DELETE FROM recovery_codes WHERE username = ?")
            .bind(username)
            .execute(&mut *tx)
            .await?;
        if secret.is_some() {
            for hash in recovery_hashes {
                sqlx::query(
                    "INSERT OR IGNORE INTO recovery_codes (username, code_hash) VALUES (?, ?)",
                )
                .bind(username)
                .bind(hash)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(done.rows_affected() == 1)
    }

    /// Records that the code of 30-second `step` was used; false if that
    /// step, or a later one, was already used (a code works once).
    pub async fn use_totp_step(&self, username: &str, step: i64) -> Result<bool, DbError> {
        let done = sqlx::query(
            "UPDATE users SET totp_step = ? WHERE username = ?              AND totp_secret IS NOT NULL AND (totp_step IS NULL OR totp_step < ?)",
        )
        .bind(step)
        .bind(username)
        .bind(step)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Uses a recovery code (SHA-256); false if it is not one of the account.
    pub async fn use_recovery_code(
        &self,
        username: &str,
        code_hash: &str,
    ) -> Result<bool, DbError> {
        let done = sqlx::query("DELETE FROM recovery_codes WHERE username = ? AND code_hash = ?")
            .bind(username)
            .bind(code_hash)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Recovery codes an account has left.
    pub async fn recovery_codes_left(&self, username: &str) -> Result<i64, DbError> {
        Ok(
            sqlx::query("SELECT COUNT(*) AS n FROM recovery_codes WHERE username = ?")
                .bind(username)
                .fetch_one(&self.pool)
                .await?
                .try_get("n")?,
        )
    }

    /// Creates an API token (SHA-256 of its secret); its identifier.
    pub async fn create_api_token(
        &self,
        username: &str,
        name: &str,
        token_hash: &str,
        read_only: bool,
        now_ms: i64,
    ) -> Result<i64, DbError> {
        Ok(sqlx::query(
            "INSERT INTO api_tokens (username, name, token_hash, read_only, created_ms)              VALUES (?, ?, ?, ?, ?)",
        )
        .bind(username)
        .bind(name)
        .bind(token_hash)
        .bind(read_only)
        .bind(now_ms)
        .execute(&self.pool)
        .await?
        .last_insert_rowid())
    }

    /// The API tokens of an account, newest first.
    pub async fn api_tokens(&self, username: &str) -> Result<Vec<ApiTokenRecord>, DbError> {
        let rows = sqlx::query(
            "SELECT id, username, name, read_only, created_ms, last_used_ms FROM api_tokens              WHERE username = ? ORDER BY id DESC",
        )
        .bind(username)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(ApiTokenRecord {
                    id: r.try_get("id")?,
                    username: r.try_get("username")?,
                    name: r.try_get("name")?,
                    read_only: r.try_get("read_only")?,
                    created_ms: r.try_get("created_ms")?,
                    last_used_ms: r.try_get("last_used_ms")?,
                })
            })
            .collect()
    }

    /// Revokes an API token of an account; false if it has no such token.
    pub async fn delete_api_token(&self, username: &str, id: i64) -> Result<bool, DbError> {
        let done = sqlx::query("DELETE FROM api_tokens WHERE id = ? AND username = ?")
            .bind(id)
            .bind(username)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Who the API token of this SHA-256 signs in as.
    pub async fn api_token_user(&self, token_hash: &str) -> Result<Option<TokenUser>, DbError> {
        let row = sqlx::query(
            "SELECT t.id, t.read_only, t.last_used_ms, u.username, u.role, u.cameras              FROM api_tokens t JOIN users u ON u.username = t.username WHERE t.token_hash = ?",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            Ok(TokenUser {
                id: r.try_get("id")?,
                username: r.try_get("username")?,
                role: r.try_get("role")?,
                cameras: cameras_from_row(&r)?,
                read_only: r.try_get("read_only")?,
                last_used_ms: r.try_get("last_used_ms")?,
            })
        })
        .transpose()
    }

    /// Records the last use of an API token.
    pub async fn touch_api_token(&self, id: i64, now_ms: i64) -> Result<(), DbError> {
        sqlx::query("UPDATE api_tokens SET last_used_ms = ? WHERE id = ?")
            .bind(now_ms)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Records a login session.
    pub async fn create_session(
        &self,
        token_hash: &str,
        username: &str,
        now_ms: i64,
        expires_ms: i64,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO sessions (token_hash, username, created_ms, expires_ms) VALUES (?, ?, ?, ?)",
        )
        .bind(token_hash)
        .bind(username)
        .bind(now_ms)
        .bind(expires_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Account, role and visible cameras (`None`: all) of a valid session.
    pub async fn session_user(
        &self,
        token_hash: &str,
        now_ms: i64,
    ) -> Result<Option<(String, String, Option<Vec<String>>)>, DbError> {
        let row = sqlx::query(
            "SELECT u.username, u.role, u.cameras FROM sessions s \
             JOIN users u ON u.username = s.username \
             WHERE s.token_hash = ? AND s.expires_ms > ?",
        )
        .bind(token_hash)
        .bind(now_ms)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            Ok((
                r.try_get("username")?,
                r.try_get("role")?,
                cameras_from_row(&r)?,
            ))
        })
        .transpose()
    }

    /// Ends a session.
    pub async fn delete_session(&self, token_hash: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Ends every session of an account (password changed, account removed).
    pub async fn delete_sessions_of(&self, username: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM sessions WHERE username = ?")
            .bind(username)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Forgets expired sessions.
    pub async fn purge_sessions(&self, now_ms: i64) -> Result<u64, DbError> {
        Ok(sqlx::query("DELETE FROM sessions WHERE expires_ms <= ?")
            .bind(now_ms)
            .execute(&self.pool)
            .await?
            .rows_affected())
    }

    /// Adds a line to the activity log of the accounts.
    pub async fn add_audit(
        &self,
        at_ms: i64,
        username: Option<&str>,
        client: &str,
        action: &str,
        status: u16,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO audit (at_ms, username, client, action, status) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(at_ms)
        .bind(username)
        .bind(client)
        .bind(action)
        .bind(i64::from(status))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The activity log, newest first: at most `limit` lines, older than the
    /// line `before` when given (to read the next page).
    pub async fn audit(&self, before: Option<i64>, limit: u32) -> Result<Vec<AuditEntry>, DbError> {
        let rows = sqlx::query(
            "SELECT id, at_ms, username, client, action, status FROM audit \
             WHERE (?1 IS NULL OR id < ?1) ORDER BY id DESC LIMIT ?2",
        )
        .bind(before)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                let status: i64 = r.try_get("status")?;
                Ok(AuditEntry {
                    id: r.try_get("id")?,
                    at_ms: r.try_get("at_ms")?,
                    username: r.try_get("username")?,
                    client: r.try_get("client")?,
                    action: r.try_get("action")?,
                    status: u16::try_from(status).unwrap_or(0),
                })
            })
            .collect()
    }

    /// Forgets the lines of the activity log older than `before_ms`, and the
    /// oldest ones beyond the `keep` most recent; returns how many went.
    pub async fn purge_audit(&self, before_ms: i64, keep: u32) -> Result<u64, DbError> {
        Ok(sqlx::query(
            "DELETE FROM audit WHERE at_ms < ?1 \
             OR id <= (SELECT id FROM audit ORDER BY id DESC LIMIT 1 OFFSET ?2)",
        )
        .bind(before_ms)
        .bind(i64::from(keep))
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    /// Camera, start and recognized object labels of every event started in
    /// `[from_ms, to_ms)`, for statistics (no thumbnails nor zones).
    pub async fn event_summaries(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<(String, i64, Vec<String>)>, DbError> {
        let rows = sqlx::query(
            "SELECT camera, start_ms, objects FROM events WHERE start_ms >= ? AND start_ms < ?",
        )
        .bind(from_ms)
        .bind(to_ms)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                let objects: String = r.try_get("objects")?;
                let mut labels: Vec<String> = serde_json::from_str::<Vec<DetectedObject>>(&objects)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|o| o.label)
                    .collect();
                labels.sort_unstable();
                labels.dedup();
                Ok((r.try_get("camera")?, r.try_get("start_ms")?, labels))
            })
            .collect()
    }

    /// Records a crossing of line `line` of `camera`.
    pub async fn add_crossing(
        &self,
        camera: &str,
        line: &str,
        forward: bool,
        at_ms: i64,
    ) -> Result<(), DbError> {
        sqlx::query("INSERT INTO crossings (camera, line, forward, at_ms) VALUES (?, ?, ?, ?)")
            .bind(camera)
            .bind(line)
            .bind(forward)
            .bind(at_ms)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Crossings in `[from_ms, to_ms)`: camera, line, forward, time.
    pub async fn crossings(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<(String, String, bool, i64)>, DbError> {
        sqlx::query(
            "SELECT camera, line, forward, at_ms FROM crossings WHERE at_ms >= ? AND at_ms < ? ORDER BY at_ms",
        )
        .bind(from_ms)
        .bind(to_ms)
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(|r| {
            Ok((
                r.try_get("camera")?,
                r.try_get("line")?,
                r.try_get("forward")?,
                r.try_get("at_ms")?,
            ))
        })
        .collect()
    }

    /// Directions of the crossings of one line since `from_ms`, in order
    /// (`true`: in the arrow direction).
    pub async fn line_crossings_since(
        &self,
        camera: &str,
        line: &str,
        from_ms: i64,
    ) -> Result<Vec<bool>, DbError> {
        sqlx::query(
            "SELECT forward FROM crossings WHERE camera = ? AND line = ? AND at_ms >= ? ORDER BY at_ms, id",
        )
        .bind(camera)
        .bind(line)
        .bind(from_ms)
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(|r| Ok(r.try_get("forward")?))
        .collect()
    }

    /// Last manual reset of the presence count of a line, if any.
    pub async fn presence_reset(&self, camera: &str, line: &str) -> Result<Option<i64>, DbError> {
        Ok(
            sqlx::query("SELECT at_ms FROM presence_resets WHERE camera = ? AND line = ?")
                .bind(camera)
                .bind(line)
                .fetch_optional(&self.pool)
                .await?
                .map(|r| r.try_get("at_ms"))
                .transpose()?,
        )
    }

    /// Records a manual reset of the presence count of a line.
    pub async fn set_presence_reset(
        &self,
        camera: &str,
        line: &str,
        at_ms: i64,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO presence_resets (camera, line, at_ms) VALUES (?, ?, ?) \
             ON CONFLICT (camera, line) DO UPDATE SET at_ms = excluded.at_ms",
        )
        .bind(camera)
        .bind(line)
        .bind(at_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Adds a bookmark.
    pub async fn add_bookmark(
        &self,
        camera: &str,
        at_ms: i64,
        note: &str,
        author: Option<&str>,
        now_ms: i64,
    ) -> Result<Bookmark, DbError> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO bookmarks (camera, at_ms, note, author, created_ms) \
             VALUES (?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(camera)
        .bind(at_ms)
        .bind(note)
        .bind(author)
        .bind(now_ms)
        .fetch_one(&self.pool)
        .await?;
        Ok(Bookmark {
            id,
            camera: camera.to_owned(),
            at_ms,
            note: note.to_owned(),
            author: author.map(str::to_owned),
            created_ms: now_ms,
        })
    }

    /// Bookmarks from `from_ms` (included) to `to_ms` (excluded), of one
    /// camera or all, oldest first, `limit` at most.
    pub async fn bookmarks(
        &self,
        from_ms: i64,
        to_ms: i64,
        camera: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Bookmark>, DbError> {
        let rows = sqlx::query(
            "SELECT id, camera, at_ms, note, author, created_ms FROM bookmarks \
             WHERE at_ms >= ? AND at_ms < ? AND (? IS NULL OR camera = ?) \
             ORDER BY at_ms LIMIT ?",
        )
        .bind(from_ms)
        .bind(to_ms)
        .bind(camera)
        .bind(camera)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| Bookmark::from_row(r).map_err(DbError::from))
            .collect()
    }

    /// One bookmark.
    pub async fn bookmark(&self, id: i64) -> Result<Option<Bookmark>, DbError> {
        sqlx::query(
            "SELECT id, camera, at_ms, note, author, created_ms FROM bookmarks WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .map(|r| Bookmark::from_row(&r).map_err(DbError::from))
        .transpose()
    }

    /// Removes a bookmark; `false` if there was none with this identifier.
    pub async fn delete_bookmark(&self, id: i64) -> Result<bool, DbError> {
        Ok(sqlx::query("DELETE FROM bookmarks WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected()
            > 0)
    }

    /// Order of the cameras chosen in the interface (identifiers, first
    /// first); empty: the order of the configuration.
    pub async fn camera_order(&self) -> Result<Vec<String>, DbError> {
        sqlx::query("SELECT camera FROM camera_order ORDER BY position")
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|r| r.try_get("camera").map_err(DbError::from))
            .collect()
    }

    /// Replaces the order of the cameras (empty: back to the configuration).
    pub async fn set_camera_order(&self, cameras: &[String]) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM camera_order")
            .execute(&mut *tx)
            .await?;
        for (position, camera) in cameras.iter().enumerate() {
            sqlx::query("INSERT INTO camera_order (camera, position) VALUES (?, ?)")
                .bind(camera)
                .bind(i64::try_from(position).unwrap_or(i64::MAX))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Settings of the video wall (JSON, as the API writes them), if chosen.
    pub async fn wall_settings(&self) -> Result<Option<String>, DbError> {
        Ok(
            sqlx::query("SELECT settings FROM wall_settings WHERE id = 1")
                .fetch_optional(&self.pool)
                .await?
                .map(|r| r.try_get("settings"))
                .transpose()?,
        )
    }

    /// Stores the settings of the video wall (JSON).
    pub async fn set_wall_settings(&self, json: &str) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO wall_settings (id, settings) VALUES (1, ?) \
             ON CONFLICT(id) DO UPDATE SET settings = excluded.settings",
        )
        .bind(json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The arrows between cameras (JSON of `JourneySettings`), if chosen.
    pub async fn journey_settings(&self) -> Result<Option<String>, DbError> {
        Ok(
            sqlx::query("SELECT settings FROM journey_settings WHERE id = 1")
                .fetch_optional(&self.pool)
                .await?
                .map(|r| r.try_get("settings"))
                .transpose()?,
        )
    }

    /// Stores the arrows between cameras (JSON).
    pub async fn set_journey_settings(&self, json: &str) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO journey_settings (id, settings) VALUES (1, ?) \
             ON CONFLICT(id) DO UPDATE SET settings = excluded.settings",
        )
        .bind(json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Puts the events `ids` into the journey `journey` (the identifier of
    /// its first event).
    pub async fn set_journey(&self, ids: &[i64], journey: i64) -> Result<(), DbError> {
        sqlx::query("UPDATE events SET journey = ?1 WHERE id IN (SELECT value FROM json_each(?2))")
            .bind(journey)
            .bind(serde_json::to_string(ids)?)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The events of a journey, in order; empty when there is none (or its
    /// events were removed by the retention).
    pub async fn journey_events(&self, journey: i64) -> Result<Vec<DetectionEvent>, DbError> {
        let rows = sqlx::query("SELECT * FROM events WHERE journey = ? ORDER BY start_ms, id")
            .bind(journey)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(event_from_row).collect()
    }

    /// Removes crossings older than `before_ms` (with the events retention).
    pub async fn delete_crossings_before(&self, before_ms: i64) -> Result<u64, DbError> {
        Ok(sqlx::query("DELETE FROM crossings WHERE at_ms < ?")
            .bind(before_ms)
            .execute(&self.pool)
            .await?
            .rows_affected())
    }

    /// Protects the recordings of `camera` between `start_ms` and `end_ms`.
    pub async fn protect(
        &self,
        camera: &str,
        start_ms: i64,
        end_ms: i64,
        note: &str,
        now_ms: i64,
    ) -> Result<ProtectedRange, DbError> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO protected_ranges (camera, start_ms, end_ms, note, created_ms) \
             VALUES (?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(camera)
        .bind(start_ms)
        .bind(end_ms)
        .bind(note)
        .bind(now_ms)
        .fetch_one(&self.pool)
        .await?;
        Ok(ProtectedRange {
            id,
            camera: camera.to_owned(),
            start_ms,
            end_ms,
            note: note.to_owned(),
            created_ms: now_ms,
        })
    }

    /// Removes a protection; `false` if there was none with this identifier.
    pub async fn unprotect(&self, id: i64) -> Result<bool, DbError> {
        Ok(sqlx::query("DELETE FROM protected_ranges WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected()
            > 0)
    }

    /// Every protected period, most recent first.
    pub async fn protected_ranges(&self) -> Result<Vec<ProtectedRange>, DbError> {
        sqlx::query("SELECT * FROM protected_ranges ORDER BY start_ms DESC")
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(protected_from_row)
            .collect()
    }

    /// Adds or replaces a segment in the index.
    pub async fn upsert_segment(&self, s: &SegmentRecord) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO segments (destination, camera, day, file, start_ms, end_ms, bytes) \n             VALUES (?, ?, ?, ?, ?, ?, ?) \n             ON CONFLICT (destination, camera, file) DO UPDATE SET \n             day = excluded.day, start_ms = excluded.start_ms, end_ms = excluded.end_ms, \n             bytes = excluded.bytes",
        )
        .bind(&s.destination)
        .bind(&s.camera)
        .bind(&s.day)
        .bind(&s.file)
        .bind(s.start_ms)
        .bind(s.end_ms)
        .bind(i64::try_from(s.bytes).unwrap_or(i64::MAX))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Segments of a camera overlapping `[from_ms, to_ms)`, by start then destination.
    pub async fn segments(
        &self,
        camera: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<SegmentRecord>, DbError> {
        let rows = sqlx::query(
            "SELECT * FROM segments WHERE camera = ? AND start_ms < ? AND end_ms > ? \n             ORDER BY start_ms, destination",
        )
        .bind(camera)
        .bind(to_ms)
        .bind(from_ms)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(segment_from_row).collect()
    }

    /// Every indexed segment of a destination (for reconciliation with the files).
    pub async fn destination_segments(
        &self,
        destination: &str,
    ) -> Result<Vec<SegmentRecord>, DbError> {
        let rows = sqlx::query("SELECT * FROM segments WHERE destination = ?")
            .bind(destination)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(segment_from_row).collect()
    }

    /// Removes a segment from the index.
    pub async fn delete_segment(
        &self,
        destination: &str,
        camera: &str,
        file: &str,
    ) -> Result<(), DbError> {
        sqlx::query("DELETE FROM segments WHERE destination = ? AND camera = ? AND file = ?")
            .bind(destination)
            .bind(camera)
            .bind(file)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Forgets the segments of destinations that no longer exist.
    pub async fn retain_destinations(&self, ids: &[String]) -> Result<u64, DbError> {
        let all: Vec<String> = sqlx::query_scalar("SELECT DISTINCT destination FROM segments")
            .fetch_all(&self.pool)
            .await?;
        let mut removed = 0;
        for d in all.into_iter().filter(|d| !ids.contains(d)) {
            removed += sqlx::query("DELETE FROM segments WHERE destination = ?")
                .bind(d)
                .execute(&self.pool)
                .await?
                .rows_affected();
        }
        Ok(removed)
    }

    /// Closes the connections (pending writes are flushed).
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustnvr_core::MotionZone;

    fn cam(id: &str) -> CameraId {
        CameraId::new(id).expect("id")
    }

    /// The checksum of each migration is kept in the databases: built from
    /// CRLF files (a Windows checkout), the program refuses every existing
    /// database. See .gitattributes.
    #[test]
    fn migrations_are_built_with_lf_line_endings() {
        for m in sqlx::migrate!("./migrations").iter() {
            assert!(
                !m.sql.as_str().contains('\r'),
                "migration {} has CRLF line endings: rebuild from an LF checkout",
                m.version
            );
        }
    }

    #[tokio::test]
    async fn events_round_trip_and_retention() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = Database::open(dir.path()).await.expect("open");
        let a = cam("a");
        let mut e1 = db
            .start_event(&a, EventKind::Motion, 1_000, 0.1, &["Allée".into()])
            .await
            .expect("start");
        e1.end_ms = Some(5_000);
        e1.score = 0.4;
        db.update_event(&e1).await.expect("update");
        db.set_thumbnail(&e1, b"\xff\xd8jpeg").await.expect("thumb");
        let person = DetectedObject {
            label: "person".into(),
            score: 0.9,
            bbox: Some([0.1, 0.2, 0.3, 0.4]),
            zone: Some("Allée".into()),
            plate: None,
            plate_owner: None,
        };
        db.set_objects(e1.id, std::slice::from_ref(&person))
            .await
            .expect("objects");
        let e2 = db
            .start_event(&cam("b"), EventKind::Motion, 10_000, 0.2, &[])
            .await
            .expect("start");

        let all = db
            .events(&EventFilter {
                cameras: None,
                camera: None,
                from_ms: 0,
                to_ms: 20_000,
                limit: 10,
                objects: None,
                zones: None,
                plate: None,
            })
            .await
            .expect("list");
        assert_eq!(
            all.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![e2.id, e1.id]
        );
        assert_eq!(all[1].zones, vec!["Allée".to_owned()]);
        assert_eq!(all[1].objects, vec![person]);
        assert!(all[0].objects.is_empty());
        assert!(all[1].has_thumbnail && (all[1].score - 0.4).abs() < 1e-6);
        // Searched by object and by zone, on the server.
        let search = |objects: Option<&[&str]>, zones: Option<&[&str]>| EventFilter {
            cameras: None,
            camera: None,
            from_ms: 0,
            to_ms: 20_000,
            limit: 10,
            objects: objects.map(|o| o.iter().map(|s| (*s).to_owned()).collect()),
            zones: zones.map(|z| z.iter().map(|s| (*s).to_owned()).collect()),
            plate: None,
        };
        let ids = |list: Vec<DetectionEvent>| list.iter().map(|e| e.id).collect::<Vec<_>>();
        let found = db
            .events(&search(Some(&["person", "dog"]), None))
            .await
            .expect("objects");
        assert_eq!(ids(found), vec![e1.id]);
        let found = db
            .events(&search(Some(&["car"]), None))
            .await
            .expect("objects");
        assert!(found.is_empty());
        let found = db
            .events(&search(None, Some(&["Allée"])))
            .await
            .expect("zones");
        assert_eq!(ids(found), vec![e1.id]);
        let found = db
            .events(&search(None, Some(&["Garage"])))
            .await
            .expect("zones");
        assert!(found.is_empty());
        // By a part of a plate read on a vehicle of the event.
        let car = DetectedObject {
            label: "car".into(),
            score: 0.8,
            bbox: Some([0.5, 0.5, 0.9, 0.9]),
            zone: None,
            plate: Some("AB123CD".into()),
            plate_owner: None,
        };
        db.set_objects(e2.id, &[car]).await.expect("objects");
        let by_plate = |plate: &str| EventFilter {
            plate: Some(plate.to_owned()),
            ..search(None, None)
        };
        let found = db.events(&by_plate("123C")).await.expect("plate");
        assert_eq!(ids(found), vec![e2.id]);
        let found = db.events(&by_plate("ZZ999")).await.expect("plate");
        assert!(found.is_empty());
        // A wildcard is never a search.
        let found = db.events(&by_plate("%")).await.expect("plate");
        assert!(found.is_empty());
        // Ongoing events overlap any later period.
        let later = db
            .events(&EventFilter {
                cameras: None,
                camera: Some(cam("b")),
                from_ms: 50_000,
                to_ms: 60_000,
                limit: 10,
                objects: None,
                zones: None,
                plate: None,
            })
            .await
            .expect("list");
        assert_eq!(later.len(), 1);
        // Overlap: [1000, 5000] is in [4000, 4500).
        let mid = db
            .events(&EventFilter {
                cameras: None,
                camera: Some(a.clone()),
                from_ms: 4_000,
                to_ms: 4_500,
                limit: 10,
                objects: None,
                zones: None,
                plate: None,
            })
            .await
            .expect("list");
        assert_eq!(mid.len(), 1);

        let thumb = db.thumbnail_path(e1.id).await.expect("path").expect("some");
        assert!(thumb.is_file());
        assert_eq!(db.delete_events_before(9_000).await.expect("delete"), 1);
        assert!(!thumb.exists());
        assert!(db.event(e1.id).await.expect("get").is_none());
        db.close().await;

        // Reopening closes the event left ongoing.
        let db = Database::open(dir.path()).await.expect("reopen");
        let e2 = db.event(e2.id).await.expect("get").expect("some");
        assert_eq!(e2.end_ms, Some(10_000));
    }

    #[tokio::test]
    async fn segment_index() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = Database::open(dir.path()).await.expect("open");
        let seg = |dest: &str, file: &str, start: i64| SegmentRecord {
            destination: dest.into(),
            camera: "a".into(),
            day: "2026-11-02".into(),
            file: file.into(),
            start_ms: start,
            end_ms: start + 60_000,
            bytes: 100,
        };
        db.upsert_segment(&seg("local", "s1.mp4", 0))
            .await
            .expect("insert");
        db.upsert_segment(&seg("nas", "s1.mp4", 0))
            .await
            .expect("insert");
        db.upsert_segment(&seg("local", "s2.mp4", 60_000))
            .await
            .expect("insert");
        db.upsert_segment(&seg("local", "s2.mp4", 60_000))
            .await
            .expect("upsert");
        let found = db.segments("a", 30_000, 70_000).await.expect("query");
        assert_eq!(
            found
                .iter()
                .map(|s| (s.destination.as_str(), s.file.as_str()))
                .collect::<Vec<_>>(),
            vec![("local", "s1.mp4"), ("nas", "s1.mp4"), ("local", "s2.mp4")]
        );
        assert!(
            db.segments("a", 120_000, 130_000)
                .await
                .expect("q")
                .is_empty()
        );
        db.delete_segment("local", "a", "s2.mp4")
            .await
            .expect("delete");
        assert_eq!(db.destination_segments("local").await.expect("q").len(), 1);
        assert_eq!(
            db.retain_destinations(&["local".into()])
                .await
                .expect("retain"),
            1
        );
        assert!(db.destination_segments("nas").await.expect("q").is_empty());
    }

    #[tokio::test]
    async fn users_and_sessions() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = Database::open(dir.path()).await.expect("open");
        assert!(
            db.create_user("alice", "h1", "admin", 1)
                .await
                .expect("create")
        );
        assert!(
            !db.create_user("alice", "h2", "viewer", 2)
                .await
                .expect("dup"),
            "taken"
        );
        assert_eq!(
            db.user_secret("alice").await.expect("get"),
            Some(("h1".into(), "admin".into()))
        );
        db.create_session("t1", "alice", 10, 100)
            .await
            .expect("session");
        assert_eq!(
            db.session_user("t1", 50).await.expect("s"),
            Some(("alice".into(), "admin".into(), None))
        );
        assert_eq!(
            db.session_user("t1", 100).await.expect("s"),
            None,
            "expired"
        );
        assert!(
            db.update_user("alice", None, Some("viewer"))
                .await
                .expect("update")
        );
        assert_eq!(
            db.session_user("t1", 50).await.expect("s").map(|u| u.1),
            Some("viewer".into())
        );
        assert_eq!(db.purge_sessions(100).await.expect("purge"), 1);
        db.create_session("t2", "alice", 10, 100)
            .await
            .expect("session");
        assert!(db.delete_user("alice").await.expect("delete"));
        assert_eq!(
            db.session_user("t2", 50).await.expect("s"),
            None,
            "sessions removed"
        );
        assert!(db.users().await.expect("list").is_empty());
    }

    #[tokio::test]
    async fn viewers_can_be_limited_to_some_cameras() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = Database::open(dir.path()).await.expect("open");
        assert!(db.create_user("bob", "h", "viewer", 1).await.expect("new"));
        assert_eq!(db.user_cameras("bob").await.expect("all"), None);
        let only = vec!["jardin".to_owned()];
        assert!(db.set_user_cameras("bob", Some(&only)).await.expect("set"));
        assert!(!db.set_user_cameras("nobody", None).await.expect("unknown"));
        assert_eq!(
            db.user_cameras("bob").await.expect("some"),
            Some(only.clone())
        );
        assert_eq!(
            db.users().await.expect("list")[0].cameras,
            Some(only.clone())
        );
        db.create_session("t", "bob", 10, 100)
            .await
            .expect("session");
        assert_eq!(
            db.session_user("t", 50).await.expect("s").map(|u| u.2),
            Some(Some(only.clone()))
        );

        // Events of the other cameras are left out, the limit counting the rest.
        for (camera, start) in [("jardin", 1_000), ("garage", 2_000), ("jardin", 3_000)] {
            db.start_event(&cam(camera), EventKind::Motion, start, 0.1, &[])
                .await
                .expect("event");
        }
        let seen = |cameras: Option<Vec<String>>, limit| {
            let db = db.clone();
            async move {
                db.events(&EventFilter {
                    camera: None,
                    cameras,
                    from_ms: 0,
                    to_ms: 10_000,
                    limit,
                    objects: None,
                    zones: None,
                    plate: None,
                })
                .await
                .expect("events")
                .iter()
                .map(|e| (e.camera.to_string(), e.start_ms))
                .collect::<Vec<_>>()
            }
        };
        assert_eq!(seen(None, 10).await.len(), 3);
        assert_eq!(
            seen(Some(only), 2).await,
            vec![("jardin".to_owned(), 3_000), ("jardin".to_owned(), 1_000)]
        );
        assert!(seen(Some(Vec::new()), 10).await.is_empty(), "no camera");

        assert!(db.set_user_cameras("bob", None).await.expect("all again"));
        assert_eq!(db.user_cameras("bob").await.expect("all"), None);
    }

    #[tokio::test]
    async fn activity_log_pages_and_is_purged() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = Database::open(dir.path()).await.expect("open");
        for (at, user, action, status) in [
            (10, Some("alice"), "login", 200),
            (20, None, "login_failed", 401),
            (30, Some("alice"), "PUT /settings", 200),
            (40, Some("bob"), "POST /cameras/jardin/export", 200),
        ] {
            db.add_audit(at, user, "192.0.2.7", action, status)
                .await
                .expect("add");
        }
        let page = db.audit(None, 3).await.expect("page");
        assert_eq!(
            page.iter().map(|e| e.at_ms).collect::<Vec<_>>(),
            [40, 30, 20]
        );
        assert_eq!((page[2].username.as_deref(), page[2].status), (None, 401));
        let next = db.audit(Some(page[2].id), 3).await.expect("next");
        assert_eq!(next.len(), 1);
        assert_eq!(
            (next[0].username.as_deref(), next[0].action.as_str()),
            (Some("alice"), "login")
        );
        // Older than 15 ms goes, then everything but the 2 most recent.
        assert_eq!(db.purge_audit(15, 100).await.expect("old"), 1);
        assert_eq!(db.purge_audit(0, 2).await.expect("many"), 1);
        let left = db.audit(None, 10).await.expect("left");
        assert_eq!(left.iter().map(|e| e.at_ms).collect::<Vec<_>>(), [40, 30]);
    }

    #[tokio::test]
    async fn crossings_round_trip() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = Database::open(dir.path()).await.expect("open");
        db.add_crossing("jardin", "portail", true, 1_000)
            .await
            .expect("add");
        db.add_crossing("jardin", "portail", false, 2_000)
            .await
            .expect("add");
        db.add_crossing("entree", "porte", true, 9_000)
            .await
            .expect("add");
        let got = db.crossings(0, 5_000).await.expect("list");
        assert_eq!(
            got,
            vec![
                ("jardin".to_owned(), "portail".to_owned(), true, 1_000),
                ("jardin".to_owned(), "portail".to_owned(), false, 2_000),
            ]
        );
        assert_eq!(
            db.line_crossings_since("jardin", "portail", 1_500)
                .await
                .expect("line"),
            vec![false]
        );
        assert_eq!(
            db.presence_reset("jardin", "portail").await.expect("reset"),
            None
        );
        db.set_presence_reset("jardin", "portail", 3_000)
            .await
            .expect("set");
        db.set_presence_reset("jardin", "portail", 4_000)
            .await
            .expect("again");
        assert_eq!(
            db.presence_reset("jardin", "portail").await.expect("reset"),
            Some(4_000)
        );
        assert_eq!(db.delete_crossings_before(5_000).await.expect("purge"), 2);
        assert_eq!(db.crossings(0, 10_000).await.expect("list").len(), 1);
    }

    #[tokio::test]
    async fn protected_ranges_round_trip() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = Database::open(dir.path()).await.expect("open");
        let a = db
            .protect("jardin", 1_000, 61_000, "colis", 5)
            .await
            .expect("protect");
        let b = db
            .protect("entree", 2_000, 3_000, "", 6)
            .await
            .expect("protect");
        let all = db.protected_ranges().await.expect("list");
        assert_eq!(all, vec![b.clone(), a.clone()]);
        assert!(db.unprotect(b.id).await.expect("unprotect"));
        assert!(!db.unprotect(b.id).await.expect("again"));
        assert_eq!(db.protected_ranges().await.expect("list"), vec![a]);
    }

    #[tokio::test]
    async fn events_left_open_end_when_rustnvr_was_last_seen() {
        let dir = tempfile::tempdir().expect("tmp");
        {
            let db = Database::open(dir.path()).await.expect("open");
            db.start_event(&cam("a"), EventKind::Motion, 10_000, 0.1, &[])
                .await
                .expect("start");
            db.start_event(&cam("b"), EventKind::Motion, 50_000, 0.1, &[])
                .await
                .expect("start");
        }
        // Stopped abruptly; last seen running at 30 s.
        let db = Database::open_after(dir.path(), Some(30_000))
            .await
            .expect("reopen");
        let all = db
            .events(&EventFilter {
                camera: None,
                cameras: None,
                from_ms: 0,
                to_ms: 100_000,
                limit: 10,
                objects: None,
                zones: None,
                plate: None,
            })
            .await
            .expect("list");
        let end = |camera: &str| {
            all.iter()
                .find(|e| e.camera.as_str() == camera)
                .and_then(|e| e.end_ms)
        };
        assert_eq!(end("a"), Some(30_000));
        // Started after the last sign of life: ends at its start.
        assert_eq!(end("b"), Some(50_000));
    }

    #[tokio::test]
    async fn bookmarks_round_trip() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = Database::open(dir.path()).await.expect("open");
        let a = db
            .add_bookmark("jardin", 2_000, "Livraison colis", Some("alice"), 9)
            .await
            .expect("add");
        db.add_bookmark("portail", 1_000, "Facteur", None, 9)
            .await
            .expect("add");
        db.add_bookmark("jardin", 5_000, "Plus tard", None, 9)
            .await
            .expect("add");
        let day = db.bookmarks(0, 5_000, None, 100).await.expect("list");
        assert_eq!(
            day.iter().map(|b| b.note.as_str()).collect::<Vec<_>>(),
            ["Facteur", "Livraison colis"]
        );
        let garden = db
            .bookmarks(0, 10_000, Some("jardin"), 100)
            .await
            .expect("list");
        assert_eq!(garden.len(), 2);
        assert_eq!(db.bookmark(a.id).await.expect("get"), Some(a.clone()));
        assert_eq!(a.author.as_deref(), Some("alice"));
        assert!(db.delete_bookmark(a.id).await.expect("delete"));
        assert!(!db.delete_bookmark(a.id).await.expect("again"));
        assert_eq!(db.bookmark(a.id).await.expect("get"), None);
    }

    #[tokio::test]
    async fn camera_order_round_trip() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = Database::open(dir.path()).await.expect("open");
        assert!(db.camera_order().await.expect("empty").is_empty());
        let order = vec![
            "portail".to_owned(),
            "jardin".to_owned(),
            "garage".to_owned(),
        ];
        db.set_camera_order(&order).await.expect("set");
        assert_eq!(db.camera_order().await.expect("get"), order);
        db.set_camera_order(&order[1..]).await.expect("replace");
        assert_eq!(db.camera_order().await.expect("get"), &order[1..]);
        db.set_camera_order(&[]).await.expect("clear");
        assert!(db.camera_order().await.expect("get").is_empty());
    }

    #[tokio::test]
    async fn two_step_login_and_api_tokens() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = Database::open(dir.path()).await.expect("open");
        assert!(db.create_user("anne", "h", "admin", 1).await.expect("user"));
        assert_eq!(db.user_totp("anne").await.expect("none"), None);

        // On, with two recovery codes; each step and each code works once.
        let codes = ["c1".to_owned(), "c2".to_owned()];
        assert!(
            db.set_user_totp("anne", Some("SECRET"), &codes)
                .await
                .expect("on")
        );
        assert_eq!(
            db.user_totp("anne").await.expect("on").as_deref(),
            Some("SECRET")
        );
        assert!(db.users().await.expect("users")[0].totp);
        assert!(db.use_totp_step("anne", 100).await.expect("step"));
        assert!(!db.use_totp_step("anne", 100).await.expect("again"));
        assert!(!db.use_totp_step("anne", 99).await.expect("older"));
        assert!(db.use_totp_step("anne", 101).await.expect("next"));
        assert!(db.use_recovery_code("anne", "c1").await.expect("code"));
        assert!(!db.use_recovery_code("anne", "c1").await.expect("used"));
        assert_eq!(db.recovery_codes_left("anne").await.expect("left"), 1);
        // Off: secret and codes forgotten.
        assert!(db.set_user_totp("anne", None, &[]).await.expect("off"));
        assert_eq!(db.user_totp("anne").await.expect("off"), None);
        assert_eq!(db.recovery_codes_left("anne").await.expect("left"), 0);
        assert!(!db.use_totp_step("anne", 200).await.expect("no secret"));

        // Tokens: found by their hash, revoked by their account only.
        let id = db
            .create_api_token("anne", "Home Assistant", "hash1", true, 5)
            .await
            .expect("token");
        let user = db
            .api_token_user("hash1")
            .await
            .expect("get")
            .expect("found");
        assert_eq!(
            (user.username.as_str(), user.role.as_str()),
            ("anne", "admin")
        );
        assert!(user.read_only && user.last_used_ms.is_none());
        db.touch_api_token(id, 9).await.expect("touch");
        let list = db.api_tokens("anne").await.expect("list");
        assert_eq!((list.len(), list[0].last_used_ms), (1, Some(9)));
        assert!(!db.delete_api_token("bob", id).await.expect("not bob's"));
        assert!(db.delete_api_token("anne", id).await.expect("revoke"));
        assert_eq!(db.api_token_user("hash1").await.expect("gone"), None);

        // Removing the account removes its tokens.
        db.create_api_token("anne", "script", "hash2", false, 6)
            .await
            .expect("token");
        assert!(db.delete_user("anne").await.expect("delete"));
        assert_eq!(db.api_token_user("hash2").await.expect("gone"), None);
    }

    #[tokio::test]
    async fn wall_settings_round_trip() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = Database::open(dir.path()).await.expect("open");
        assert_eq!(db.wall_settings().await.expect("empty"), None);
        db.set_wall_settings(r#"{"cameras":["a"]}"#)
            .await
            .expect("set");
        db.set_wall_settings(r#"{"cameras":["b"]}"#)
            .await
            .expect("replace");
        assert_eq!(
            db.wall_settings().await.expect("get").as_deref(),
            Some(r#"{"cameras":["b"]}"#)
        );
    }

    #[tokio::test]
    async fn journeys_group_events() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = Database::open(dir.path()).await.expect("open");
        assert_eq!(db.journey_settings().await.expect("empty"), None);
        db.set_journey_settings(r#"{"links":[]}"#)
            .await
            .expect("set");
        assert_eq!(
            db.journey_settings().await.expect("get").as_deref(),
            Some(r#"{"links":[]}"#)
        );
        let a = db
            .start_event(&cam("sonnette"), EventKind::Motion, 1_000, 0.2, &[])
            .await
            .expect("a");
        let b = db
            .start_event(&cam("entree"), EventKind::Motion, 9_000, 0.2, &[])
            .await
            .expect("b");
        let other = db
            .start_event(&cam("jardin"), EventKind::Motion, 5_000, 0.2, &[])
            .await
            .expect("other");
        db.set_journey(&[a.id, b.id], a.id).await.expect("join");
        let journey = db.journey_events(a.id).await.expect("journey");
        let cams: Vec<&str> = journey.iter().map(|e| e.camera.as_str()).collect();
        assert_eq!(cams, ["sonnette", "entree"]);
        assert!(journey.iter().all(|e| e.journey == Some(a.id)));
        assert_eq!(
            db.event(other.id)
                .await
                .expect("get")
                .expect("some")
                .journey,
            None
        );
        assert!(db.journey_events(other.id).await.expect("none").is_empty());
    }

    #[tokio::test]
    async fn deleting_an_event_keeps_its_journey_together() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = Database::open(dir.path()).await.expect("open");
        let mut ids = Vec::new();
        for (camera, start) in [("sonnette", 1_000), ("entree", 9_000), ("garage", 20_000)] {
            let mut e = db
                .start_event(&cam(camera), EventKind::Motion, start, 0.2, &[])
                .await
                .expect("start");
            e.end_ms = Some(start + 5_000);
            db.update_event(&e).await.expect("end");
            ids.push(e.id);
        }
        db.set_journey(&ids, ids[0]).await.expect("join");
        let ongoing = db
            .start_event(&cam("jardin"), EventKind::Motion, 30_000, 0.2, &[])
            .await
            .expect("ongoing");
        assert_eq!(
            db.delete_event(ongoing.id).await.expect("del"),
            EventDeletion::Ongoing
        );
        assert_eq!(
            db.delete_event(999).await.expect("del"),
            EventDeletion::Missing
        );

        // The first one goes: the journey is named after the next one.
        assert_eq!(
            db.delete_event(ids[0]).await.expect("del"),
            EventDeletion::Deleted
        );
        assert!(db.event(ids[0]).await.expect("get").is_none());
        let journey = db.journey_events(ids[1]).await.expect("journey");
        assert_eq!(journey.len(), 2);
        // One event left: no journey any more.
        assert_eq!(
            db.delete_event(ids[2]).await.expect("del"),
            EventDeletion::Deleted
        );
        let left = db.event(ids[1]).await.expect("get").expect("kept");
        assert_eq!(left.journey, None);
    }

    #[tokio::test]
    async fn motion_settings_round_trip() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = Database::open(dir.path()).await.expect("open");
        let s = MotionSettings {
            enabled: true,
            sensitivity: 70,
            zones: vec![MotionZone {
                name: "Allée".into(),
                points: vec![[0.0, 0.0], [1.0, 0.0], [0.5, 1.0]],
                sensitivity: 40,
                objects: Vec::new(),
            }],
            objects: vec!["person".into()],
            masks: Vec::new(),
            lines: Vec::new(),
            schedule: Vec::new(),
            mute_notifications: false,
            timelapse: false,
            camera_photo: false,
            camera_photo_width: 1920,
            no_picture_check: false,
        };
        db.set_motion_settings(&cam("a"), &s).await.expect("set");
        db.set_motion_settings(&cam("a"), &s).await.expect("upsert");
        assert_eq!(db.motion_settings().await.expect("get").get("a"), Some(&s));
        db.delete_motion_settings(&cam("a")).await.expect("delete");
        assert!(db.motion_settings().await.expect("get").is_empty());
        // The JSON accepted by the API.
        let json = r#"{"enabled":true,"sensitivity":60,"zones":[{"name":"Z","points":[[0,0],[1,0],[0.5,1]],"sensitivity":40}]}"#;
        let parsed: MotionSettings = serde_json::from_str(json).expect("json");
        assert!(parsed.problems().is_empty());
        assert!(!parsed.mute_notifications);
        // Muted notifications are kept; unmuted, the field is not written,
        // so that an older version still reads the settings.
        let muted = MotionSettings {
            mute_notifications: true,
            ..s.clone()
        };
        db.set_motion_settings(&cam("b"), &muted)
            .await
            .expect("set");
        assert_eq!(
            db.motion_settings().await.expect("get").get("b"),
            Some(&muted)
        );
        assert!(!serde_json::to_string(&s).expect("json").contains("mute"));
    }
}
