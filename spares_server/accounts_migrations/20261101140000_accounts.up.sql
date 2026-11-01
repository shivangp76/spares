-- A host's accounts (see `spares_server::host::accounts`). Collections never refer to these, so a
-- user's directory can move between hosts.
CREATE TABLE user (
    -- Names the user's directory, `users/<id>`
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    email TEXT UNIQUE COLLATE NOCASE,
    -- Argon2id, in the PHC string format
    password_hash TEXT NOT NULL,
    admin INTEGER NOT NULL DEFAULT 0,
    -- Unix times
    created_at INTEGER NOT NULL,
    email_verified_at INTEGER,
    disabled_at INTEGER
) STRICT;
