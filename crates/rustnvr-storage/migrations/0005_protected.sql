-- Periods of recordings protected by the user: retention and the free-space
-- reserve never delete the segments they cover.
CREATE TABLE protected_ranges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    camera TEXT NOT NULL,
    start_ms INTEGER NOT NULL,
    end_ms INTEGER NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    created_ms INTEGER NOT NULL
);
CREATE INDEX protected_ranges_camera ON protected_ranges (camera, start_ms);
