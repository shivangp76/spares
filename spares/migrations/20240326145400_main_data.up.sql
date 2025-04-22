-- sqlx migrate revert && sqlx migrate run

---- Create the 'state' table
--CREATE TABLE IF NOT EXISTS state (
--    id INTEGER PRIMARY KEY NOT NULL,
--    name TEXT NOT NULL
--);
--
---- Insert data into the 'state' table
--INSERT INTO state (id, name) VALUES (0, 'New');
--INSERT INTO state (id, name) VALUES (1, 'Learning');
--INSERT INTO state (id, name) VALUES (2, 'Review');
--INSERT INTO state (id, name) VALUES (3, 'Relearning');
--
---- Create the 'rating' table
--CREATE TABLE IF NOT EXISTS rating (
--    id INTEGER PRIMARY KEY NOT NULL,
--    name TEXT NOT NULL
--);
--
---- Insert data into the 'rating' table
--INSERT INTO rating (id, name) VALUES (1, 'Again');
--INSERT INTO rating (id, name) VALUES (2, 'Hard');
--INSERT INTO rating (id, name) VALUES (3, 'Good');
--INSERT INTO rating (id, name) VALUES (4, 'Easy');

-- Ensure all foreign keys are valid
PRAGMA foreign_keys = ON;

-- Create the 'parser' table
CREATE TABLE IF NOT EXISTS parser (
    id INTEGER PRIMARY KEY NOT NULL,
    name VARCHAR NOT NULL
);

-- Create the 'tag' table
CREATE TABLE IF NOT EXISTS tag (
    id INTEGER PRIMARY KEY NOT NULL,
    name VARCHAR NOT NULL,
    description TEXT NOT NULL,
    parent_id INTEGER, -- ID of parent tag
    query TEXT,
    auto_delete BOOLEAN NOT NULL,
    FOREIGN KEY (parent_id) REFERENCES tag(id)
);

-- Create the 'note' table
CREATE TABLE IF NOT EXISTS note (
    id INTEGER PRIMARY KEY NOT NULL,
    data TEXT NOT NULL,
    keywords TEXT NOT NULL,
    created_at INTEGER DEFAULT (strftime('%s', 'now')) NOT NULL, -- Store as Unix Time
    updated_at INTEGER DEFAULT (strftime('%s', 'now')) NOT NULL, -- Store as Unix Time
    custom_data TEXT NOT NULL, -- JSON string
    parser_id INTEGER NOT NULL, -- Foreign key to 'parser' table
    FOREIGN KEY (parser_id) REFERENCES parser(id)
);

-- Create the 'card' table
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
    -- elapsed_days INTEGER NOT NULL,
    -- scheduled_days INTEGER NOT NULL,
    -- reps INTEGER NOT NULL,
    -- lapses INTEGER NOT NULL,
    special_state INTEGER, -- Enum
    state INTEGER NOT NULL, -- Foreign key to 'state' table
    -- last_review INTEGER NOT NULL, -- Store as Unix Time
    -- previous_state INTEGER NOT NULL,
    -- review_log_id INTEGER NOT NULL, -- Foreign key to 'review_log' table
    custom_data TEXT NOT NULL, -- JSON string
    FOREIGN KEY (note_id) REFERENCES note(id) ON DELETE CASCADE -- When note is deleted, delete all corresponding cards
    --FOREIGN KEY (state_id) REFERENCES state(id),
    --FOREIGN KEY (review_log_id) REFERENCES review_log(id)
);

-- Create the 'note_link' table
CREATE TABLE IF NOT EXISTS note_link (
    id INTEGER PRIMARY KEY NOT NULL,
    parent_note_id INTEGER NOT NULL,
    linked_note_id INTEGER,
    "order" INTEGER NOT NULL,
    searched_keyword VARCHAR NOT NULL,
    matched_keyword VARCHAR,
    FOREIGN KEY (parent_note_id) REFERENCES note(id) ON DELETE CASCADE, -- When note is deleted, delete all corresponding linked notes
    FOREIGN KEY (linked_note_id) REFERENCES note(id)
    -- PRIMARY KEY (parent_note_id, linked_note_id)
);

-- Create the 'note_tag' table
CREATE TABLE IF NOT EXISTS note_tag (
    id INTEGER PRIMARY KEY NOT NULL,
    note_id INTEGER NOT NULL,
    tag_id INTEGER NOT NULL,
    FOREIGN KEY (note_id) REFERENCES note(id) ON DELETE CASCADE, -- When note is deleted, delete its corresponding note_tag entry
    FOREIGN KEY (tag_id) REFERENCES tag(id) ON DELETE CASCADE -- When tag is deleted, delete its corresponding note_tag entry
    -- PRIMARY KEY (note_id, tag_id)
);

-- Create the 'card_tag' table
CREATE TABLE IF NOT EXISTS card_tag (
    id INTEGER PRIMARY KEY NOT NULL,
    card_id INTEGER NOT NULL,
    tag_id INTEGER NOT NULL,
    FOREIGN KEY (card_id) REFERENCES card(id) ON DELETE CASCADE, -- When card is deleted, delete its corresponding card_tag entry
    FOREIGN KEY (tag_id) REFERENCES tag(id) ON DELETE CASCADE -- When tag is deleted, delete its corresponding card_tag entry
    -- PRIMARY KEY (card_id, tag_id)
);

-- Create the 'scheduler' table
-- CREATE TABLE IF NOT EXISTS scheduler (
--     id INTEGER PRIMARY KEY NOT NULL,
--     name VARCHAR NOT NULL
-- );

-- Create the 'review_log' table
CREATE TABLE IF NOT EXISTS review_log (
    id INTEGER PRIMARY KEY NOT NULL,
    card_id INTEGER NOT NULL,
    reviewed_at INTEGER DEFAULT (strftime('%s', 'now')) NOT NULL, -- Store as Unix Time
    rating INTEGER NOT NULL,
    -- scheduler_id INTEGER NOT NULL,
    scheduler_name TEXT NOT NULL,
    scheduled_time INTEGER NOT NULL,
    duration INTEGER NOT NULL,
    -- elapsed_time INTEGER NOT NULL,
    previous_state INTEGER NOT NULL,
    custom_data TEXT NOT NULL, -- JSON string <https://docs.rs/sqlx/latest/sqlx/sqlite/types/index.html#json>
    FOREIGN KEY (card_id) REFERENCES card(id) ON DELETE CASCADE
    -- FOREIGN KEY (scheduler_id) REFERENCES scheduler(id)
    --FOREIGN KEY (rating_id) REFERENCES rating(id),
    --FOREIGN KEY (state_id) REFERENCES state(id)
);
