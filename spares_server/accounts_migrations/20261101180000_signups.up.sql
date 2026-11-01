-- Signups (see `spares_server::host::signup`)

-- Invites an admin made (`spares host invite`), for signups = "invite"
CREATE TABLE invite (
    code_hash TEXT PRIMARY KEY NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    used_by TEXT REFERENCES user (id) ON DELETE SET NULL,
    used_at INTEGER
) STRICT;

-- Links sent by email: to verify an address, or to reset a password
CREATE TABLE email_token (
    token_hash TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES user (id) ON DELETE CASCADE,
    purpose TEXT NOT NULL CHECK (purpose IN ('verify', 'reset')),
    expires_at INTEGER NOT NULL
) STRICT;
