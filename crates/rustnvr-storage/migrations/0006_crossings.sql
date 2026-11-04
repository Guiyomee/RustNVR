-- Crossings of the lines drawn on cameras (`forward`: 1 from the left of the
-- line as drawn to its right, 0 the other way).
CREATE TABLE crossings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    camera TEXT NOT NULL,
    line TEXT NOT NULL,
    forward INTEGER NOT NULL,
    at_ms INTEGER NOT NULL
);
CREATE INDEX crossings_at ON crossings (at_ms);
