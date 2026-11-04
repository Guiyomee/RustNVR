-- Objects recognized in the event thumbnail by the optional AI provider
-- (JSON array of {label, score, bbox?}).
ALTER TABLE events ADD COLUMN objects TEXT NOT NULL DEFAULT '[]';
