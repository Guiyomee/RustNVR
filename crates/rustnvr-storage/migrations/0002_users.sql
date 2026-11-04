-- Accounts of the web interface. Passwords are stored as Argon2 PHC strings.
CREATE TABLE users (
    username TEXT PRIMARY KEY,
    password_hash TEXT NOT NULL,
    -- 'admin' (everything) or 'viewer' (watch only).
    role TEXT NOT NULL,
    created_ms INTEGER NOT NULL
);

-- Login sessions. Only a SHA-256 of the session token is kept.
CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    username TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    expires_ms INTEGER NOT NULL
);
CREATE INDEX sessions_username ON sessions (username);
