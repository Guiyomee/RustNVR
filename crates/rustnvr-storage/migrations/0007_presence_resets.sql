-- Last manual reset of the presence count of a line (the count also starts
-- again from zero at local midnight).
CREATE TABLE presence_resets (
    camera TEXT NOT NULL,
    line TEXT NOT NULL,
    at_ms INTEGER NOT NULL,
    PRIMARY KEY (camera, line)
);
