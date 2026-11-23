-- Two-step login: the TOTP secret of an account (base32), set once the
-- account confirmed a first code; NULL: the password alone.
ALTER TABLE users ADD COLUMN totp_secret TEXT;
-- Last 30-second step whose code was accepted: a code works once.
ALTER TABLE users ADD COLUMN totp_step INTEGER;

-- Recovery codes of the two-step login, each usable once (SHA-256 only).
CREATE TABLE recovery_codes (
    username TEXT NOT NULL,
    code_hash TEXT NOT NULL,
    PRIMARY KEY (username, code_hash)
);

-- API tokens: a device or a script signs in as an account without its
-- password (SHA-256 of the token only).
CREATE TABLE api_tokens (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    -- 1: reading only, even for an administrator.
    read_only INTEGER NOT NULL,
    created_ms INTEGER NOT NULL,
    last_used_ms INTEGER
);
CREATE INDEX api_tokens_username ON api_tokens (username);
