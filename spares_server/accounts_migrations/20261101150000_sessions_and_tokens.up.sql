-- Logins (see `spares_server::host::auth`). Secrets are stored as their blake3 hashes.

-- The web app's cookie sessions
CREATE TABLE session (
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
CREATE INDEX session_user ON session (user_id);

-- Devices: the CLI, and other hosts whose copy of a collection syncs with this one
CREATE TABLE device_token (
    id INTEGER PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    -- `all`, or `sync` for a host that only syncs
    scope TEXT NOT NULL DEFAULT 'all' CHECK (scope IN ('all', 'sync')),
    -- The replica of the collection the device enrolled as, which revoking the token frees
    replica INTEGER,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at INTEGER
) STRICT;
CREATE INDEX device_token_user ON device_token (user_id);
