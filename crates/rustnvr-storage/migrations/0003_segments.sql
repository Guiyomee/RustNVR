-- Index of finished recording segments (rebuilt from the files when needed:
-- the files remain the reference).
CREATE TABLE segments (
    destination TEXT NOT NULL,
    camera TEXT NOT NULL,
    -- `YYYY-MM-DD` directory and file name of the segment.
    day TEXT NOT NULL,
    file TEXT NOT NULL,
    start_ms INTEGER NOT NULL,
    end_ms INTEGER NOT NULL,
    bytes INTEGER NOT NULL,
    PRIMARY KEY (destination, camera, file)
);
CREATE INDEX segments_camera_start ON segments (camera, start_ms);
