-- Bookmarks on the timeline: a moment of a camera with a note, to find it
-- again without watching everything (« livraison colis »).
CREATE TABLE bookmarks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    camera TEXT NOT NULL,
    at_ms INTEGER NOT NULL,
    note TEXT NOT NULL,
    author TEXT,
    created_ms INTEGER NOT NULL
);
CREATE INDEX bookmarks_at ON bookmarks (at_ms);
