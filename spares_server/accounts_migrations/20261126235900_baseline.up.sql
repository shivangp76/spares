-- A host's accounts as of 2026-11-26: every table is STRICT and has no defaults, so every insert
-- says what it writes. It replaces the migrations up to `20261126230000_baseline`, which made this
-- schema on a new `accounts.sqlite` and converted older ones to it (see the git history for them).
--
-- An `accounts.sqlite` those migrations made already has all of this, so every statement is
-- `IF NOT EXISTS`, and `accounts::open` runs the migrations with `ignore_missing`, which accepts
-- the old ones in `_sqlx_migrations`. One that didn't run the last of them is refused: opening it
-- with the spares of 2026-11-26 first brings it up to date.

-- The failed constraint names what to do.
CREATE TEMP TABLE baseline_guard (
    open_this_host_with_the_spares_of_2026_11_26_first INTEGER NOT NULL
);
INSERT INTO baseline_guard
SELECT NULL
WHERE EXISTS (SELECT 1 FROM _sqlx_migrations WHERE version < 20261126235900)
    AND NOT EXISTS (SELECT 1 FROM _sqlx_migrations WHERE version = 20261126230000 AND success);
DROP TABLE baseline_guard;

-- A host's accounts (see `spares_server::host::accounts`). Collections never refer to these, so a
-- user's directory can move between hosts.
CREATE TABLE IF NOT EXISTS user (
    -- Names the user's directory, `users/<id>`
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    email TEXT UNIQUE COLLATE NOCASE,
    -- Argon2id, in the PHC string format
    password_hash TEXT NOT NULL,
    admin INTEGER NOT NULL CHECK (admin IN (0, 1)),
    -- Unix times
    created_at INTEGER NOT NULL,
    email_verified_at INTEGER,
    disabled_at INTEGER
) STRICT;

-- Devices: the CLI, and other hosts whose copy of a collection syncs with this one
CREATE TABLE IF NOT EXISTS device_token (
    id INTEGER PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    -- `all`, or `sync` for a host that only syncs
    scope TEXT NOT NULL CHECK (scope IN ('all', 'sync')),
    -- The replica of the collection the device enrolled as, which revoking the token frees
    replica INTEGER,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at INTEGER
) STRICT;

CREATE INDEX IF NOT EXISTS device_token_user ON device_token (user_id);

-- Logins (see `spares_server::host::auth`). Secrets are stored as their blake3 hashes.

-- The web app's cookie sessions
CREATE TABLE IF NOT EXISTS session (
    token_hash TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    -- Sent back in `X-CSRF-Token` with every request that changes something
    csrf_token TEXT NOT NULL,
    user_agent TEXT,
    -- Unix times. Each use moves `expires_at` on.
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS session_user ON session (user_id);

-- What a user may use, where it differs from host.toml's [limits] (NULL: the host's), and what
-- they use (see `spares_server::host::limits`).
CREATE TABLE IF NOT EXISTS user_limits (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    sync_body_mb INTEGER,
    media_body_mb INTEGER,
    storage_mb INTEGER,
    requests_per_minute INTEGER,
    search_seconds INTEGER,
    renders_at_once INTEGER
) STRICT;

CREATE TABLE IF NOT EXISTS usage (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    -- The size of the user's directory when last measured, and when (Unix time)
    storage_bytes INTEGER NOT NULL,
    storage_measured_at INTEGER,
    -- Requests since the account was created
    requests INTEGER NOT NULL,
    last_request_at INTEGER
) STRICT;

-- Signups (see `spares_server::host::signup`)

-- Invites an admin made (`spares host invite`), for signups = "invite"
CREATE TABLE IF NOT EXISTS invite (
    code_hash TEXT PRIMARY KEY NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_by TEXT REFERENCES user (id) ON DELETE SET NULL,
    used_at INTEGER
) STRICT;

-- Links sent by email: to verify an address, or to reset a password
CREATE TABLE IF NOT EXISTS email_token (
    token_hash TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    purpose TEXT NOT NULL CHECK (purpose IN ('verify', 'reset')),
    expires_at INTEGER NOT NULL
) STRICT;
