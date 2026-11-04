-- Detection events. Times are milliseconds since the Unix epoch (UTC).
CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    camera TEXT NOT NULL,
    kind TEXT NOT NULL,
    start_ms INTEGER NOT NULL,
    -- NULL while the event is ongoing.
    end_ms INTEGER,
    -- Largest fraction of a zone (or of the picture) that changed, 0..1.
    score REAL NOT NULL DEFAULT 0,
    -- JSON array of zone names.
    zones TEXT NOT NULL DEFAULT '[]',
    -- Thumbnail file name in the thumbnails directory, if any.
    thumbnail TEXT
);
CREATE INDEX events_start ON events (start_ms);
CREATE INDEX events_camera_start ON events (camera, start_ms);

-- Motion detection settings of each camera (JSON, see MotionSettings).
CREATE TABLE motion_settings (
    camera TEXT PRIMARY KEY,
    settings TEXT NOT NULL
);
