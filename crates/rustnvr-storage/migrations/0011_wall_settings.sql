-- Settings of the video wall chosen in the interface, the same on every
-- device: JSON of the API's wall settings, in a single row.
CREATE TABLE wall_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    settings TEXT NOT NULL
);
