-- Cameras a viewer account may see: NULL for all of them, else a JSON array
-- of camera identifiers. Ignored for administrators.
ALTER TABLE users ADD COLUMN cameras TEXT;

-- Activity of the accounts: logins (failed ones too), logouts and every
-- request that changes something, with the account and the client address.
CREATE TABLE audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at_ms INTEGER NOT NULL,
    -- Account (NULL: none known, as for a failed login of an unknown name).
    username TEXT,
    -- Address of the client.
    client TEXT NOT NULL,
    -- `login`, `login_failed`, `logout`, or `METHOD /path` of a request.
    action TEXT NOT NULL,
    -- HTTP status of the answer.
    status INTEGER NOT NULL
);
CREATE INDEX audit_at ON audit (at_ms);
