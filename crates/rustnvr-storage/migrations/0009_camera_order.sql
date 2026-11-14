-- Order of the cameras chosen in the interface, the same on every device.
-- Cameras not listed come after, in the order of the configuration.
CREATE TABLE camera_order (
    camera TEXT PRIMARY KEY,
    position INTEGER NOT NULL
);
