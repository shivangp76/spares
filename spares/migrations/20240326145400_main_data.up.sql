-- sqlx migrate revert && sqlx migrate run

-- Ensure all foreign keys are valid
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS parser (
    id INTEGER PRIMARY KEY NOT NULL,
    name VARCHAR NOT NULL
);

CREATE TABLE IF NOT EXISTS tag (
    id INTEGER PRIMARY KEY NOT NULL,
    name VARCHAR NOT NULL,
    description TEXT NOT NULL,
    parent_id INTEGER, -- ID of parent tag
    query TEXT,
    auto_delete INTEGER NOT NULL CHECK (auto_delete IN (0, 1)),
    -- To prevent cascades wiping large tag trees, but still keep all references valid.
    FOREIGN KEY (parent_id) REFERENCES tag(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS note (
    id INTEGER PRIMARY KEY NOT NULL,
    data TEXT NOT NULL,
    created_at INTEGER DEFAULT (strftime('%s', 'now')) NOT NULL, -- Store as Unix Time
    updated_at INTEGER DEFAULT (strftime('%s', 'now')) NOT NULL, -- Store as Unix Time
    custom_data TEXT NOT NULL, -- JSON string
    parser_id INTEGER NOT NULL, -- Foreign key to 'parser' table
    -- Prevent deleting `parser_id` if notes depend on it
    FOREIGN KEY (parser_id) REFERENCES parser(id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS note_keyword (
    note_id INTEGER NOT NULL,
    keyword VARCHAR NOT NULL,
    embedded INTEGER NOT NULL CHECK (embedded IN (0, 1)),
    FOREIGN KEY (note_id) REFERENCES note(id) ON DELETE CASCADE -- When note is deleted, delete all corresponding keywords
);

CREATE TABLE IF NOT EXISTS card (
    id INTEGER PRIMARY KEY NOT NULL,
    note_id INTEGER NOT NULL,
    "order" INTEGER NOT NULL,
    back_type INTEGER NOT NULL, -- Enum
    created_at INTEGER DEFAULT (strftime('%s', 'now')) NOT NULL, -- Store as Unix Time
    updated_at INTEGER DEFAULT (strftime('%s', 'now')) NOT NULL, -- Store as Unix Time
    due INTEGER NOT NULL, -- Store as Unix Time
    stability REAL NOT NULL,
    difficulty REAL NOT NULL,
    desired_retention REAL NOT NULL,
    special_state INTEGER, -- Enum
    state INTEGER NOT NULL, -- Foreign key to 'state' table
    custom_data TEXT NOT NULL, -- JSON string
    FOREIGN KEY (note_id) REFERENCES note(id) ON DELETE CASCADE -- When note is deleted, delete all corresponding cards
);

CREATE TABLE IF NOT EXISTS note_link (
    id INTEGER PRIMARY KEY NOT NULL,
    parent_note_id INTEGER NOT NULL,
    linked_note_id INTEGER,
    "order" INTEGER NOT NULL,
    searched_keyword VARCHAR NOT NULL,
    matched_keyword VARCHAR,
    score REAL,
    FOREIGN KEY (parent_note_id) REFERENCES note(id) ON DELETE CASCADE, -- When note is deleted, delete all corresponding linked notes
    FOREIGN KEY (linked_note_id) REFERENCES note(id) ON DELETE SET NULL
    -- PRIMARY KEY (parent_note_id, linked_note_id)
);

CREATE TABLE IF NOT EXISTS note_tag (
    id INTEGER PRIMARY KEY NOT NULL,
    note_id INTEGER NOT NULL,
    tag_id INTEGER NOT NULL,
    FOREIGN KEY (note_id) REFERENCES note(id) ON DELETE CASCADE, -- When note is deleted, delete its corresponding note_tag entry
    FOREIGN KEY (tag_id) REFERENCES tag(id) ON DELETE CASCADE -- When tag is deleted, delete its corresponding note_tag entry
    UNIQUE (note_id, tag_id)
    -- PRIMARY KEY (note_id, tag_id)
);

CREATE TABLE IF NOT EXISTS card_tag (
    id INTEGER PRIMARY KEY NOT NULL,
    card_id INTEGER NOT NULL,
    tag_id INTEGER NOT NULL,
    FOREIGN KEY (card_id) REFERENCES card(id) ON DELETE CASCADE, -- When card is deleted, delete its corresponding card_tag entry
    FOREIGN KEY (tag_id) REFERENCES tag(id) ON DELETE CASCADE -- When tag is deleted, delete its corresponding card_tag entry
    UNIQUE (card_id, tag_id)
);

CREATE TABLE IF NOT EXISTS review_log (
    id INTEGER PRIMARY KEY NOT NULL,
    card_id INTEGER,
    reviewed_at INTEGER DEFAULT (strftime('%s', 'now')) NOT NULL, -- Store as Unix Time
    rating INTEGER NOT NULL,
    scheduler_name TEXT NOT NULL,
    scheduled_time INTEGER NOT NULL,
    recall_duration INTEGER NOT NULL,
    rate_duration INTEGER NOT NULL,
    previous_state INTEGER NOT NULL,
    custom_data TEXT NOT NULL, -- JSON string <https://docs.rs/sqlx/latest/sqlx/sqlite/types/index.html#json>
    -- Do _NOT_ delete review logs when cards are deleted. We want to know how many cards were reviewed in the past for historical reasons. Instead, set the `card_id` column to null, to signify the row is an orphan.
    FOREIGN KEY (card_id) REFERENCES card(id) ON DELETE SET NULL
);

-- Add indexes to optimize JOIN operations in note rendering queries

-- Index on note.parser_id for faster parser joins
CREATE INDEX IF NOT EXISTS idx_note_parser_id ON note(parser_id);

-- Indexes on note_tag for faster tag joins
CREATE INDEX IF NOT EXISTS idx_note_tag_note_id ON note_tag(note_id);
CREATE INDEX IF NOT EXISTS idx_note_tag_tag_id ON note_tag(tag_id);

-- Index on note_link.parent_note_id for faster note link lookups
CREATE INDEX IF NOT EXISTS idx_note_link_parent_note_id ON note_link(parent_note_id);

-- Indexes on note_keyword for faster keyword lookups
CREATE INDEX IF NOT EXISTS idx_note_keyword_note_id ON note_keyword(note_id);
CREATE INDEX IF NOT EXISTS idx_note_keyword_keyword ON note_keyword(keyword);

-- Index on tag.query for faster filtering in WHERE clause
CREATE INDEX IF NOT EXISTS idx_tag_query ON tag(query);
