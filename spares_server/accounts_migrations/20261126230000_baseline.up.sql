-- no-transaction
-- A host's accounts as of 2026-11-26, which replaces the migrations from `20261101140000_accounts`
-- to `20261101180000_signups`, and the baseline of 2026-11-24 that had replaced them (see the git
-- history for them). Every table is STRICT and has no defaults, so every insert says what it
-- writes.
--
-- A new `accounts.sqlite` gets its tables from here, and so does one those migrations made:
-- `accounts::open` runs the migrations with `ignore_missing`, which accepts the old ones in
-- `_sqlx_migrations`. Such a database has the tables already, and `user`, `device_token` and
-- `usage` had defaults, so those three are rebuilt: their rows are copied to a temporary table, and
-- each is dropped, created again and filled from that copy (on a new database, from an empty
-- stand-in). `user` is referred to by every other table, so foreign keys are off meanwhile, which
-- only takes effect outside a transaction: hence `no-transaction`, and a transaction of its own,
-- which checks them before committing. One that stopped partway through the migrations before
-- 2026-11-24 is refused: opening it with a spares from before 2026-11-24 first finishes them.

PRAGMA foreign_keys = OFF;

BEGIN IMMEDIATE;

-- The failed constraint names what to do.
CREATE TEMP TABLE baseline_guard (
    open_this_host_with_a_spares_from_before_2026_11_24_first INTEGER NOT NULL
);
INSERT INTO baseline_guard
SELECT NULL
WHERE EXISTS (SELECT 1 FROM _sqlx_migrations WHERE version < 20261126230000)
    AND NOT EXISTS (
        SELECT 1 FROM _sqlx_migrations
        WHERE version IN (20261101180000, 20261124110000) AND success
    );
DROP TABLE baseline_guard;

-- A host's accounts (see `spares_server::host::accounts`). Collections never refer to these, so a
-- user's directory can move between hosts.

-- As a database those migrations made has them, or on a new database as empty stand-ins, which
-- the tables below replace
CREATE TABLE IF NOT EXISTS user (
    id, name, email, password_hash, admin, created_at, email_verified_at, disabled_at
);
CREATE TABLE IF NOT EXISTS device_token (
    id, user_id, name, token_hash, scope, replica, created_at, last_used_at, revoked_at
);
CREATE TABLE IF NOT EXISTS usage (
    user_id, storage_bytes, storage_measured_at, requests, last_request_at
);

CREATE TEMP TABLE old_user AS
SELECT id, name, email, password_hash, admin, created_at, email_verified_at, disabled_at FROM user;
DROP TABLE user;
CREATE TABLE user (
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
INSERT INTO user (id, name, email, password_hash, admin, created_at, email_verified_at, disabled_at)
SELECT id, name, email, password_hash, admin, created_at, email_verified_at, disabled_at
FROM old_user;
DROP TABLE old_user;

CREATE TEMP TABLE old_device_token AS
SELECT id, user_id, name, token_hash, scope, replica, created_at, last_used_at, revoked_at
FROM device_token;
DROP TABLE device_token;
-- Devices: the CLI, and other hosts whose copy of a collection syncs with this one
CREATE TABLE device_token (
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
INSERT INTO device_token
    (id, user_id, name, token_hash, scope, replica, created_at, last_used_at, revoked_at)
SELECT id, user_id, name, token_hash, scope, replica, created_at, last_used_at, revoked_at
FROM old_device_token;
DROP TABLE old_device_token;
CREATE INDEX device_token_user ON device_token (user_id);

CREATE TEMP TABLE old_usage AS
SELECT user_id, storage_bytes, storage_measured_at, requests, last_request_at FROM usage;
DROP TABLE usage;
CREATE TABLE usage (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    -- The size of the user's directory when last measured, and when (Unix time)
    storage_bytes INTEGER NOT NULL,
    storage_measured_at INTEGER,
    -- Requests since the account was created
    requests INTEGER NOT NULL,
    last_request_at INTEGER
) STRICT;
INSERT INTO usage (user_id, storage_bytes, storage_measured_at, requests, last_request_at)
SELECT user_id, storage_bytes, storage_measured_at, requests, last_request_at FROM old_usage;
DROP TABLE old_usage;

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

-- Refuses to commit if a row refers to one that doesn't exist. The failed constraint names it.
CREATE TEMP TABLE no_defaults_guard (a_foreign_key_is_violated INTEGER NOT NULL);
INSERT INTO no_defaults_guard SELECT NULL FROM pragma_foreign_key_check LIMIT 1;
DROP TABLE no_defaults_guard;

COMMIT;

PRAGMA foreign_keys = ON;
