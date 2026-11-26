-- no-transaction
-- Drops the last defaults, so that every insert says what it writes: `user.admin` (which now
-- checks it is a boolean), `device_token.scope` and `usage`'s counts. SQLite can't change a table's
-- definition in place, so each of these tables is copied to a temporary table, dropped, created
-- again and filled from that copy, as the collection's strict schema does (see its migration).
-- `user` is referred to by every other table, so foreign keys are off meanwhile, which only takes
-- effect outside a transaction: hence `no-transaction`, and a transaction of its own, which checks
-- them before committing.

PRAGMA foreign_keys = OFF;

BEGIN IMMEDIATE;

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

-- Refuses to commit if a row refers to one that doesn't exist. The failed constraint names it.
CREATE TEMP TABLE no_defaults_guard (a_foreign_key_is_violated INTEGER NOT NULL);
INSERT INTO no_defaults_guard SELECT NULL FROM pragma_foreign_key_check LIMIT 1;
DROP TABLE no_defaults_guard;

COMMIT;

PRAGMA foreign_keys = ON;
