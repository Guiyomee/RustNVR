-- Journeys between cameras: the events of one journey carry the identifier
-- of its first event (NULL: in no journey).
ALTER TABLE events ADD COLUMN journey INTEGER;
CREATE INDEX events_journey ON events (journey) WHERE journey IS NOT NULL;

-- Arrows between cameras chosen in the interface (JSON of
-- rustnvr_core::journey::JourneySettings), in a single row.
CREATE TABLE journey_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    settings TEXT NOT NULL
);
