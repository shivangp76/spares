-- no-transaction
-- Rebuilds every table as a STRICT table, without defaults: a value of the wrong type is refused
-- rather than stored, and every insert says what it writes. The types say what the columns hold
-- (`event.kind` held integers under `STRING`, and `TEXT` replaces `VARCHAR`), and the boolean,
-- enum and JSON columns check their values. `note_fts` is a virtual table, which can't be STRICT.
--
-- SQLite can't change a table's definition in place, so each table's rows are copied to a temporary
-- table, and the table is dropped, created again and filled from that copy, keeping its rows' ids
-- (and rowids, for the tables without an integer primary key). Renaming a new table over the old
-- one would copy the rows once, but every rename reloads the whole schema, which made this
-- migration several times slower on a new database. Dropping a table with foreign keys on would
-- delete what refers to it (cascading to a note's cards, for example), so foreign keys are off
-- meanwhile, which only takes effect outside a transaction: hence `no-transaction`, and a
-- transaction of its own. The views and triggers are dropped first and made again last, once every
-- table they read exists again, and the foreign keys are checked before committing.

PRAGMA foreign_keys = OFF;

BEGIN IMMEDIATE;

DROP VIEW "event_action";
DROP VIEW "sync_note_snapshot";
DROP VIEW "sync_card_snapshot";
DROP TRIGGER "sync_parser_update";
DROP TRIGGER "sync_note_update";
DROP TRIGGER "sync_card_update";
DROP TRIGGER "sync_note_tag_insert";
DROP TRIGGER "sync_note_tag_delete";
DROP TRIGGER "sync_note_keyword_insert";
DROP TRIGGER "sync_note_keyword_delete";
DROP TRIGGER "sync_card_tag_insert";
DROP TRIGGER "sync_card_tag_delete";
DROP TRIGGER "sync_parser_before_delete";
DROP TRIGGER "sync_note_before_delete";
DROP TRIGGER "sync_card_before_delete";
DROP TRIGGER "sync_parser_delete";
DROP TRIGGER "sync_tag_delete";
DROP TRIGGER "sync_note_delete";
DROP TRIGGER "sync_card_delete";
DROP TRIGGER "sync_parser_insert";
DROP TRIGGER "sync_tag_insert";
DROP TRIGGER "sync_note_insert";
DROP TRIGGER "sync_card_insert";
DROP TRIGGER "sync_tag_update";
DROP TRIGGER "sync_tag_before_delete";
DROP TRIGGER "review_log_previous_interval_insert";
DROP TRIGGER "review_log_previous_interval_delete";
DROP TRIGGER "review_log_previous_interval_update";
DROP TRIGGER "note_fts_insert";
DROP TRIGGER "note_fts_update";
DROP TRIGGER "note_fts_delete";
DROP TRIGGER "note_fts_keyword_insert";
DROP TRIGGER "note_fts_keyword_delete";
DROP TRIGGER "note_fts_keyword_update";
DROP TRIGGER "sync_saved_search_update";
DROP TRIGGER "sync_saved_search_before_delete";
DROP TRIGGER "sync_saved_search_delete";
DROP TRIGGER "sync_saved_search_insert";
DROP TRIGGER "sync_review_log_delete";
DROP TRIGGER "sync_review_log_insert";
DROP TRIGGER "sync_received_parser_insert";
DROP TRIGGER "sync_received_parser_update";
DROP TRIGGER "sync_received_parser_delete";
DROP TRIGGER "sync_received_tag_insert";
DROP TRIGGER "sync_received_tag_update";
DROP TRIGGER "sync_received_tag_delete";
DROP TRIGGER "sync_received_note_insert";
DROP TRIGGER "sync_received_note_update";
DROP TRIGGER "sync_received_note_delete";
DROP TRIGGER "sync_received_card_insert";
DROP TRIGGER "sync_received_card_update";
DROP TRIGGER "sync_received_card_delete";
DROP TRIGGER "sync_received_note_tag_insert";
DROP TRIGGER "sync_received_note_tag_delete";
DROP TRIGGER "sync_received_card_tag_insert";
DROP TRIGGER "sync_received_card_tag_delete";
DROP TRIGGER "sync_received_note_keyword_insert";
DROP TRIGGER "sync_received_note_keyword_delete";
DROP TRIGGER "keyword_version_insert";
DROP TRIGGER "keyword_version_delete";
DROP TRIGGER "keyword_version_update";
DROP TRIGGER "study_day_insert";
DROP TRIGGER "study_day_delete";
DROP TRIGGER "study_day_update";
DROP VIEW "note_prerequisite";

CREATE TEMP TABLE old_parser AS SELECT id, name, usn FROM "parser";
DROP TABLE "parser";
CREATE TABLE "parser" (
    id INTEGER PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    -- See `spares_core::sync`.
    usn INTEGER NOT NULL
) STRICT;
INSERT INTO "parser" (id, name, usn) SELECT id, name, usn FROM old_parser;
DROP TABLE old_parser;

CREATE TEMP TABLE old_tag AS SELECT id, name, description, query, auto_delete, created_at, updated_at, usn, reschedule FROM "tag";
DROP TABLE "tag";
CREATE TABLE "tag" (
    id INTEGER PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    query TEXT,
    auto_delete INTEGER NOT NULL CHECK (auto_delete IN (0, 1)),
    created_at INTEGER NOT NULL, -- Store as Unix Time
    updated_at INTEGER NOT NULL, -- Store as Unix Time
    -- See `spares_core::sync`.
    usn INTEGER NOT NULL,
    -- Whether rating a card under this filtered tag reschedules it (see `Tag::reschedule`)
    reschedule INTEGER NOT NULL CHECK (reschedule IN (0, 1))
) STRICT;
INSERT INTO "tag" (id, name, description, query, auto_delete, created_at, updated_at, usn, reschedule) SELECT id, name, description, query, auto_delete, created_at, updated_at, usn, reschedule FROM old_tag;
DROP TABLE old_tag;

CREATE TEMP TABLE old_note AS SELECT id, data, created_at, updated_at, custom_data, parser_id, usn FROM "note";
DROP TABLE "note";
CREATE TABLE "note" (
    id INTEGER PRIMARY KEY NOT NULL,
    data TEXT NOT NULL,
    created_at INTEGER NOT NULL, -- Store as Unix Time
    updated_at INTEGER NOT NULL, -- Store as Unix Time
    custom_data TEXT NOT NULL CHECK (json_valid(custom_data)),
    parser_id INTEGER NOT NULL, -- Foreign key to 'parser' table
    -- See `spares_core::sync`.
    usn INTEGER NOT NULL,
    -- Prevent deleting `parser_id` if notes depend on it
    FOREIGN KEY (parser_id) REFERENCES parser(id) ON DELETE RESTRICT
) STRICT;
INSERT INTO "note" (id, data, created_at, updated_at, custom_data, parser_id, usn) SELECT id, data, created_at, updated_at, custom_data, parser_id, usn FROM old_note;
DROP TABLE old_note;

CREATE TEMP TABLE old_note_keyword AS SELECT rowid AS old_rowid, note_id, keyword, embedded FROM "note_keyword";
DROP TABLE "note_keyword";
CREATE TABLE "note_keyword" (
    note_id INTEGER NOT NULL,
    keyword TEXT NOT NULL,
    embedded INTEGER NOT NULL CHECK (embedded IN (0, 1)),
    FOREIGN KEY (note_id) REFERENCES note(id) ON DELETE CASCADE -- When note is deleted, delete all corresponding keywords
) STRICT;
INSERT INTO "note_keyword" (rowid, note_id, keyword, embedded) SELECT old_rowid, note_id, keyword, embedded FROM old_note_keyword;
DROP TABLE old_note_keyword;

CREATE TEMP TABLE old_card AS SELECT id, note_id, "order", back_type, created_at, updated_at, due, stability, difficulty, desired_retention, special_state, state, custom_data, usn, stability_fast FROM "card";
DROP TABLE "card";
CREATE TABLE "card" (
    id INTEGER PRIMARY KEY NOT NULL,
    note_id INTEGER NOT NULL,
    "order" INTEGER NOT NULL,
    back_type INTEGER NOT NULL, -- Enum
    created_at INTEGER NOT NULL, -- Store as Unix Time
    updated_at INTEGER NOT NULL, -- Store as Unix Time
    due INTEGER NOT NULL, -- Store as Unix Time
    stability REAL NOT NULL,
    difficulty REAL NOT NULL,
    desired_retention REAL NOT NULL,
    special_state INTEGER, -- Enum
    state INTEGER NOT NULL, -- Foreign key to 'state' table
    custom_data TEXT NOT NULL CHECK (json_valid(custom_data)),
    -- See `spares_core::sync`.
    usn INTEGER NOT NULL,
    -- FSRS-7 remembers two stabilities: the slow one in `stability` and a fast one that same-day
    -- reviews move. Under FSRS-6 the fast one equals `stability`.
    stability_fast REAL NOT NULL,
    FOREIGN KEY (note_id) REFERENCES note(id) ON DELETE CASCADE -- When note is deleted, delete all corresponding cards
) STRICT;
INSERT INTO "card" (id, note_id, "order", back_type, created_at, updated_at, due, stability, difficulty, desired_retention, special_state, state, custom_data, usn, stability_fast) SELECT id, note_id, "order", back_type, created_at, updated_at, due, stability, difficulty, desired_retention, special_state, state, custom_data, usn, stability_fast FROM old_card;
DROP TABLE old_card;

CREATE TEMP TABLE old_note_link AS SELECT id, parent_note_id, linked_note_id, "order", searched_keyword, matched_keyword, score, relation FROM "note_link";
DROP TABLE "note_link";
CREATE TABLE "note_link" (
    id INTEGER PRIMARY KEY NOT NULL,
    parent_note_id INTEGER NOT NULL,
    linked_note_id INTEGER,
    "order" INTEGER NOT NULL,
    searched_keyword TEXT NOT NULL,
    matched_keyword TEXT,
    score REAL,
    -- Which way the link points (`LinkRelation`): 0 when the note with the link builds on the
    -- note it links to, 1 when the linked note builds on it, 2 when neither does.
    relation INTEGER NOT NULL CHECK (relation IN (0, 1, 2)),
    FOREIGN KEY (parent_note_id) REFERENCES note(id) ON DELETE CASCADE, -- When note is deleted, delete all corresponding linked notes
    FOREIGN KEY (linked_note_id) REFERENCES note(id) ON DELETE SET NULL
    -- PRIMARY KEY (parent_note_id, linked_note_id)
) STRICT;
INSERT INTO "note_link" (id, parent_note_id, linked_note_id, "order", searched_keyword, matched_keyword, score, relation) SELECT id, parent_note_id, linked_note_id, "order", searched_keyword, matched_keyword, score, relation FROM old_note_link;
DROP TABLE old_note_link;

CREATE TEMP TABLE old_note_tag AS SELECT id, note_id, tag_id FROM "note_tag";
DROP TABLE "note_tag";
CREATE TABLE "note_tag" (
    id INTEGER PRIMARY KEY NOT NULL,
    note_id INTEGER NOT NULL,
    tag_id INTEGER NOT NULL,
    FOREIGN KEY (note_id) REFERENCES note(id) ON DELETE CASCADE, -- When note is deleted, delete its corresponding note_tag entry
    FOREIGN KEY (tag_id) REFERENCES tag(id) ON DELETE CASCADE -- When tag is deleted, delete its corresponding note_tag entry
) STRICT;
INSERT INTO "note_tag" (id, note_id, tag_id) SELECT id, note_id, tag_id FROM old_note_tag;
DROP TABLE old_note_tag;

CREATE TEMP TABLE old_card_tag AS SELECT id, card_id, tag_id FROM "card_tag";
DROP TABLE "card_tag";
CREATE TABLE "card_tag" (
    id INTEGER PRIMARY KEY NOT NULL,
    card_id INTEGER NOT NULL,
    tag_id INTEGER NOT NULL,
    FOREIGN KEY (card_id) REFERENCES card(id) ON DELETE CASCADE, -- When card is deleted, delete its corresponding card_tag entry
    FOREIGN KEY (tag_id) REFERENCES tag(id) ON DELETE CASCADE -- When tag is deleted, delete its corresponding card_tag entry
) STRICT;
INSERT INTO "card_tag" (id, card_id, tag_id) SELECT id, card_id, tag_id FROM old_card_tag;
DROP TABLE old_card_tag;

CREATE TEMP TABLE old_app_state AS SELECT key, value FROM "app_state";
DROP TABLE "app_state";
CREATE TABLE "app_state" (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
) STRICT;
INSERT INTO "app_state" (key, value) SELECT key, value FROM old_app_state;
DROP TABLE old_app_state;

CREATE TEMP TABLE old_render_asset AS SELECT path, hash, mtime, size FROM "render_asset";
DROP TABLE "render_asset";
CREATE TABLE "render_asset" (
    path TEXT PRIMARY KEY NOT NULL,
    hash TEXT NOT NULL, -- blake3 hex of the contents
    mtime INTEGER NOT NULL, -- Nanoseconds since the Unix epoch. Used to skip rehashing unchanged files.
    size INTEGER NOT NULL
) STRICT;
INSERT INTO "render_asset" (path, hash, mtime, size) SELECT path, hash, mtime, size FROM old_render_asset;
DROP TABLE old_render_asset;

CREATE TEMP TABLE old_render_package AS SELECT spec FROM "render_package";
DROP TABLE "render_package";
CREATE TABLE "render_package" (
    spec TEXT PRIMARY KEY NOT NULL
) STRICT;
INSERT INTO "render_package" (spec) SELECT spec FROM old_render_package;
DROP TABLE old_render_package;

CREATE TEMP TABLE old_event AS SELECT id, kind, created_at, group_id, reverts_event_id, payload FROM "event";
DROP TABLE "event";
CREATE TABLE "event" (
    id INTEGER PRIMARY KEY NOT NULL,
    kind INTEGER NOT NULL,
    -- Unix time
    created_at INTEGER NOT NULL,
    group_id INTEGER,
    reverts_event_id INTEGER UNIQUE,
    payload TEXT NOT NULL CHECK (json_valid(payload)),
    FOREIGN KEY (reverts_event_id)
        REFERENCES event(id)
) STRICT;
INSERT INTO "event" (id, kind, created_at, group_id, reverts_event_id, payload) SELECT id, kind, created_at, group_id, reverts_event_id, payload FROM old_event;
DROP TABLE old_event;

CREATE TEMP TABLE old_id_counter AS SELECT replica, tbl, last FROM "id_counter";
DROP TABLE "id_counter";
CREATE TABLE "id_counter" (
    replica INTEGER NOT NULL,
    tbl TEXT NOT NULL,
    last INTEGER NOT NULL,
    PRIMARY KEY (replica, tbl)
) STRICT;
INSERT INTO "id_counter" (replica, tbl, last) SELECT replica, tbl, last FROM old_id_counter;
DROP TABLE old_id_counter;

CREATE TEMP TABLE old_sync_conflict AS SELECT id, note_id, base, local, remote, created_at FROM "sync_conflict";
DROP TABLE "sync_conflict";
CREATE TABLE "sync_conflict" (
    id INTEGER PRIMARY KEY NOT NULL,
    note_id INTEGER NOT NULL,
    base TEXT,
    local TEXT NOT NULL,
    remote TEXT NOT NULL,
    created_at INTEGER NOT NULL -- Unix time
) STRICT;
INSERT INTO "sync_conflict" (id, note_id, base, local, remote, created_at) SELECT id, note_id, base, local, remote, created_at FROM old_sync_conflict;
DROP TABLE old_sync_conflict;

CREATE TEMP TABLE old_sync_applying AS SELECT rowid AS old_rowid, applying FROM "sync_applying";
DROP TABLE "sync_applying";
CREATE TABLE "sync_applying" (
    applying INTEGER NOT NULL
) STRICT;
INSERT INTO "sync_applying" (rowid, applying) SELECT old_rowid, applying FROM old_sync_applying;
DROP TABLE old_sync_applying;

CREATE TEMP TABLE old_sync_client AS SELECT replica, last_usn, last_sync_at FROM "sync_client";
DROP TABLE "sync_client";
CREATE TABLE "sync_client" (
    replica INTEGER PRIMARY KEY NOT NULL,
    last_usn INTEGER NOT NULL,
    last_sync_at INTEGER NOT NULL -- Unix time
) STRICT;
INSERT INTO "sync_client" (replica, last_usn, last_sync_at) SELECT replica, last_usn, last_sync_at FROM old_sync_client;
DROP TABLE old_sync_client;

CREATE TEMP TABLE old_review_log AS SELECT id, card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time, recall_duration, rate_duration, previous_state, tag_id, custom_data, usn, previous_interval FROM "review_log";
DROP TABLE "review_log";
CREATE TABLE "review_log" (
    id INTEGER PRIMARY KEY NOT NULL,
    card_id INTEGER,
    reviewed_at INTEGER NOT NULL, -- Store as Unix Time
    -- Enum. 0 = Review, 1 = Forget, 16 = Cram. See `ReviewLogKind` in `model.rs`.
    -- 0..=15 are replayed into a card's memory state, 16..=31 only recorded; the rest of each
    -- range is reserved for future kinds.
    kind INTEGER NOT NULL CHECK (kind BETWEEN 0 AND 31),
    rating INTEGER,          -- NULL unless `kind` is Review or Cram
    scheduler_name TEXT NOT NULL,
    scheduled_time INTEGER,  -- NULL unless `kind` is Review
    recall_duration INTEGER, -- NULL unless `kind` is Review or Cram
    rate_duration INTEGER,   -- NULL unless `kind` is Review or Cram
    previous_state INTEGER NOT NULL,
    -- The filtered tag this row was produced under, if any.
    tag_id INTEGER,
    custom_data TEXT NOT NULL CHECK (json_valid(custom_data)),
    -- See `spares_core::sync`.
    usn INTEGER NOT NULL,
    -- The `scheduled_time` of the card's review before this one, for a review
    previous_interval INTEGER,
    -- A review or a cram always has a rating; other kinds never do.
    CHECK ((kind IN (0, 16)) = (rating IS NOT NULL)),
    -- Do _NOT_ delete review logs when cards are deleted. We want to know how many cards were
    -- reviewed in the past for historical reasons. Instead, set the `card_id` column to null, to
    -- signify the row is an orphan.
    FOREIGN KEY (card_id) REFERENCES card(id) ON DELETE SET NULL,
    -- Likewise, deleting a filtered tag must not delete the reviews done under it.
    FOREIGN KEY (tag_id) REFERENCES tag(id) ON DELETE SET NULL
) STRICT;
INSERT INTO "review_log" (id, card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time, recall_duration, rate_duration, previous_state, tag_id, custom_data, usn, previous_interval) SELECT id, card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time, recall_duration, rate_duration, previous_state, tag_id, custom_data, usn, previous_interval FROM old_review_log;
DROP TABLE old_review_log;

CREATE TEMP TABLE old_saved_search AS SELECT id, name, query, created_at, updated_at, usn FROM "saved_search";
DROP TABLE "saved_search";
CREATE TABLE "saved_search" (
    id INTEGER PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    query TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- See `spares_core::sync`.
    usn INTEGER NOT NULL
) STRICT;
INSERT INTO "saved_search" (id, name, query, created_at, updated_at, usn) SELECT id, name, query, created_at, updated_at, usn FROM old_saved_search;
DROP TABLE old_saved_search;

CREATE TEMP TABLE old_sync_base AS SELECT kind, oid, snapshot FROM "sync_base";
DROP TABLE "sync_base";
CREATE TABLE "sync_base" (
    kind TEXT NOT NULL CHECK (kind IN ('parser', 'tag', 'note', 'card', 'saved_search')),
    oid INTEGER NOT NULL,
    snapshot TEXT NOT NULL CHECK (json_valid(snapshot)),
    PRIMARY KEY (kind, oid)
) STRICT;
INSERT INTO "sync_base" (kind, oid, snapshot) SELECT kind, oid, snapshot FROM old_sync_base;
DROP TABLE old_sync_base;

CREATE TEMP TABLE old_grave AS SELECT oid, kind, usn FROM "grave";
DROP TABLE "grave";
CREATE TABLE "grave" (
    oid INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('parser', 'tag', 'note', 'card', 'saved_search', 'review_log')),
    usn INTEGER NOT NULL,
    PRIMARY KEY (oid, kind)
) STRICT;
INSERT INTO "grave" (oid, kind, usn) SELECT oid, kind, usn FROM old_grave;
DROP TABLE old_grave;

CREATE TEMP TABLE old_sync_received AS SELECT kind, oid, event_id FROM "sync_received";
DROP TABLE "sync_received";
CREATE TABLE "sync_received" (
    kind TEXT NOT NULL,
    oid INTEGER NOT NULL,
    event_id INTEGER NOT NULL,
    PRIMARY KEY (kind, oid)
) STRICT;
INSERT INTO "sync_received" (kind, oid, event_id) SELECT kind, oid, event_id FROM old_sync_received;
DROP TABLE old_sync_received;

CREATE TEMP TABLE old_sync_device AS SELECT replica, name, enrolled_at FROM "sync_device";
DROP TABLE "sync_device";
CREATE TABLE "sync_device" (
    replica INTEGER PRIMARY KEY NOT NULL CHECK (replica BETWEEN 0 AND 7),
    name TEXT NOT NULL,
    enrolled_at INTEGER NOT NULL -- Unix time
) STRICT;
INSERT INTO "sync_device" (replica, name, enrolled_at) SELECT replica, name, enrolled_at FROM old_sync_device;
DROP TABLE old_sync_device;

CREATE TEMP TABLE old_render_output AS SELECT note_id, output, key, dependencies FROM "render_output";
DROP TABLE "render_output";
CREATE TABLE "render_output" (
    note_id INTEGER NOT NULL,
    -- '' for the note's own file, '<card order>-front' or '<card order>-back' for a card's
    output TEXT NOT NULL,
    key TEXT NOT NULL,
    -- `RenderDependencies` as JSON: {"files": [...], "packages": [...]}
    dependencies TEXT NOT NULL CHECK (json_valid(dependencies)),
    PRIMARY KEY (note_id, output),
    FOREIGN KEY (note_id) REFERENCES note(id) ON DELETE CASCADE
) STRICT;
INSERT INTO "render_output" (note_id, output, key, dependencies) SELECT note_id, output, key, dependencies FROM old_render_output;
DROP TABLE old_render_output;

-- Replaced whenever a keyword changes, so that the search's keyword matcher (`KeywordCache`) knows
-- when to read them again, including after changes by other processes or a full sync.
CREATE TEMP TABLE old_keyword_version AS SELECT rowid AS old_rowid, token FROM "keyword_version";
DROP TABLE "keyword_version";
CREATE TABLE "keyword_version" (token BLOB NOT NULL) STRICT;
INSERT INTO "keyword_version" (rowid, token) SELECT old_rowid, token FROM old_keyword_version;
DROP TABLE old_keyword_version;

-- Study days, derived locally for the configured day boundary and answer-time cap. Card presets
-- are joined at read time so moving a note between presets is immediately visible.
CREATE TEMP TABLE old_study_day_cache AS SELECT id, since, rules FROM "study_day_cache";
DROP TABLE "study_day_cache";
CREATE TABLE "study_day_cache" (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    since INTEGER NOT NULL,
    rules TEXT NOT NULL
) STRICT;
INSERT INTO "study_day_cache" (id, since, rules) SELECT id, since, rules FROM old_study_day_cache;
DROP TABLE old_study_day_cache;

CREATE TEMP TABLE old_study_day AS SELECT card_id, day, state, reviews, seconds FROM "study_day";
DROP TABLE "study_day";
CREATE TABLE "study_day" (
    card_id INTEGER NOT NULL,
    day INTEGER NOT NULL,
    state INTEGER NOT NULL,
    reviews INTEGER NOT NULL,
    seconds INTEGER NOT NULL,
    PRIMARY KEY (card_id, day)
) STRICT, WITHOUT ROWID;
INSERT INTO "study_day" (card_id, day, state, reviews, seconds) SELECT card_id, day, state, reviews, seconds FROM old_study_day;
DROP TABLE old_study_day;

-- Keep timestamps here: the SQLite trigger does not know the collection's local day.
CREATE TEMP TABLE old_study_day_dirty AS SELECT card_id, reviewed_at FROM "study_day_dirty";
DROP TABLE "study_day_dirty";
CREATE TABLE "study_day_dirty" (
    card_id INTEGER NOT NULL,
    reviewed_at INTEGER NOT NULL,
    PRIMARY KEY (card_id, reviewed_at)
) STRICT, WITHOUT ROWID;
INSERT INTO "study_day_dirty" (card_id, reviewed_at) SELECT card_id, reviewed_at FROM old_study_day_dirty;
DROP TABLE old_study_day_dirty;

-- Study actions a client named with an id (`SubmitStudyActionRequest::action_id`), so that sending
-- one again, such as after its response was lost, returns what the first attempt did rather than
-- rating or burying the card twice. Each collection has its own table, so ids are scoped to it.
-- `request` is the action as JSON, to tell a retry from a different action reusing the id, and
-- `response` what it returned. Rows older than 30 days are deleted as actions are submitted.
CREATE TEMP TABLE old_study_action AS SELECT id, request, response, created_at FROM "study_action";
DROP TABLE "study_action";
CREATE TABLE "study_action" (
    id TEXT PRIMARY KEY NOT NULL,
    request TEXT NOT NULL CHECK (json_valid(request)),
    response TEXT NOT NULL CHECK (json_valid(response)),
    created_at INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
INSERT INTO "study_action" (id, request, response, created_at) SELECT id, request, response, created_at FROM old_study_action;
DROP TABLE old_study_action;

CREATE INDEX idx_note_parser_id ON note(parser_id);

CREATE INDEX idx_note_tag_note_id ON note_tag(note_id);

CREATE INDEX idx_note_tag_tag_id ON note_tag(tag_id);

CREATE INDEX idx_note_link_parent_note_id ON note_link(parent_note_id);

CREATE INDEX idx_note_keyword_note_id ON note_keyword(note_id);

CREATE INDEX idx_note_keyword_keyword ON note_keyword(keyword);

CREATE INDEX idx_tag_query ON tag(query);

CREATE UNIQUE INDEX idx_note_live_sync
    ON note(
        json_extract(custom_data, '$.live_sync_name'),
        json_extract(custom_data, '$.live_block_order')
    );

CREATE INDEX idx_event_group_id
    ON event(group_id);

CREATE INDEX idx_event_created_at
    ON event(created_at);

CREATE INDEX idx_event_kind
    ON event(kind);

CREATE INDEX idx_parser_usn ON parser(usn);

CREATE INDEX idx_tag_usn ON tag(usn);

CREATE INDEX idx_note_usn ON note(usn);

CREATE INDEX idx_card_usn ON card(usn);

CREATE INDEX idx_card_note_id ON card(note_id);

CREATE INDEX idx_card_tag_card_id ON card_tag(card_id);

CREATE INDEX idx_sync_conflict_note_id ON sync_conflict(note_id);

CREATE INDEX idx_review_log_card_id_reviewed_at ON review_log(card_id, reviewed_at);

CREATE INDEX idx_review_log_reviewed_at ON review_log(reviewed_at);

CREATE INDEX idx_review_log_tag_id ON review_log(tag_id);

CREATE INDEX idx_review_log_usn ON review_log(usn);

-- The queue reads the previous state and both durations for eligible cards, so this index covers
-- them. id is the chronological tie-breaker of latest-review lookups when timestamps are equal.
CREATE INDEX idx_review_log_card_id_kind_reviewed_at ON review_log(
    card_id, kind, reviewed_at, id, previous_state, recall_duration, rate_duration
);

CREATE INDEX idx_saved_search_usn ON saved_search(usn);

-- A note or card has a tag once: the code adds tags with `INSERT OR IGNORE`.
CREATE UNIQUE INDEX idx_note_tag_note_id_tag_id ON note_tag(note_id, tag_id);

CREATE UNIQUE INDEX idx_card_tag_card_id_tag_id ON card_tag(card_id, tag_id);

-- Broad keyword searches test membership once per candidate note.
CREATE INDEX idx_note_keyword_note_keyword ON note_keyword(note_id, keyword);

CREATE INDEX study_day_recent ON study_day(state, day DESC, card_id);

-- A dependent link is followed from the note it links to, so its rows are found by that note.
CREATE INDEX idx_note_link_linked_note_id ON note_link(linked_note_id);

CREATE INDEX study_action_created_at ON study_action (created_at);


CREATE VIEW event_action AS
WITH RECURSIVE chain(id, depth) AS (
    SELECT id, 0 FROM event
    WHERE reverts_event_id IS NULL
    UNION ALL
    SELECT e.id, c.depth + 1
    FROM event e
    JOIN chain c ON e.reverts_event_id = c.id
)
SELECT id,
    CASE WHEN depth = 0 THEN 0
         WHEN depth % 2 = 1 THEN 1
         ELSE 2 END AS action
FROM chain;

CREATE VIEW sync_note_snapshot AS
SELECT n.id AS id, json_object(
    'id', n.id,
    'data', n.data,
    'created_at', n.created_at,
    'updated_at', n.updated_at,
    'custom_data', json(n.custom_data),
    'parser_id', n.parser_id,
    'tags', (SELECT json_group_array(tag_id) FROM
        (SELECT tag_id FROM note_tag WHERE note_id = n.id ORDER BY tag_id)),
    'keywords', (SELECT json_group_array(keyword) FROM
        (SELECT keyword FROM note_keyword WHERE note_id = n.id AND embedded = 0 ORDER BY keyword))
) AS snapshot
FROM note n;

CREATE VIEW sync_card_snapshot AS
SELECT c.id AS id, json_object(
    'id', c.id,
    'note_id', c.note_id,
    'order', c."order",
    'back_type', c.back_type,
    'created_at', c.created_at,
    'updated_at', c.updated_at,
    'due', c.due,
    'stability', c.stability,
    'stability_fast', c.stability_fast,
    'difficulty', c.difficulty,
    'desired_retention', c.desired_retention,
    'special_state', c.special_state,
    'state', c.state,
    'custom_data', json(c.custom_data),
    'tags', (SELECT json_group_array(tag_id) FROM
        (SELECT tag_id FROM card_tag WHERE card_id = c.id ORDER BY tag_id))
) AS snapshot
FROM card c;

-- Every link as a (note, prerequisite) pair: a prerequisite link (relation 0) points from the note
-- with it to the note it links to, a dependent link (1) the other way, and a related link (2)
-- makes no pair. Only links that matched a note are pairs. The queue's holds, the walks of
-- `prerequisite_of` and `dependent_of`, the note graph's arrows and goal projections all read
-- this, so a link's relation is turned into a direction in one place.
CREATE VIEW note_prerequisite AS
SELECT parent_note_id AS note_id, linked_note_id AS prerequisite_id, score, id AS link_id
FROM note_link
WHERE relation = 0 AND linked_note_id IS NOT NULL
UNION ALL
SELECT linked_note_id AS note_id, parent_note_id AS prerequisite_id, score, id AS link_id
FROM note_link
WHERE relation = 1 AND linked_note_id IS NOT NULL;

CREATE TRIGGER sync_parser_update AFTER UPDATE ON parser
WHEN NEW.usn = OLD.usn AND OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
    AND OLD.name IS NOT NEW.name
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('parser', OLD.id, json_object('id', OLD.id, 'name', OLD.name));
    UPDATE parser SET usn = -1 WHERE id = NEW.id;
END;

CREATE TRIGGER sync_note_update AFTER UPDATE ON note
WHEN NEW.usn = OLD.usn AND OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.data IS NOT NEW.data OR OLD.custom_data IS NOT NEW.custom_data
        OR OLD.parser_id IS NOT NEW.parser_id OR OLD.created_at IS NOT NEW.created_at)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('note', OLD.id, json_object(
        'id', OLD.id,
        'data', OLD.data,
        'created_at', OLD.created_at,
        'updated_at', OLD.updated_at,
        'custom_data', json(OLD.custom_data),
        'parser_id', OLD.parser_id,
        'tags', (SELECT json_group_array(tag_id) FROM
            (SELECT tag_id FROM note_tag WHERE note_id = OLD.id ORDER BY tag_id)),
        'keywords', (SELECT json_group_array(keyword) FROM
            (SELECT keyword FROM note_keyword
             WHERE note_id = OLD.id AND embedded = 0 ORDER BY keyword))
    ));
    UPDATE note SET usn = -1 WHERE id = NEW.id;
END;

CREATE TRIGGER sync_card_update AFTER UPDATE ON card
WHEN NEW.usn = OLD.usn AND OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.note_id IS NOT NEW.note_id OR OLD."order" IS NOT NEW."order"
        OR OLD.back_type IS NOT NEW.back_type OR OLD.due IS NOT NEW.due
        OR OLD.stability IS NOT NEW.stability OR OLD.stability_fast IS NOT NEW.stability_fast
        OR OLD.difficulty IS NOT NEW.difficulty
        OR OLD.desired_retention IS NOT NEW.desired_retention
        OR OLD.special_state IS NOT NEW.special_state OR OLD.state IS NOT NEW.state
        OR OLD.custom_data IS NOT NEW.custom_data OR OLD.created_at IS NOT NEW.created_at)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('card', OLD.id, json_object(
        'id', OLD.id,
        'note_id', OLD.note_id,
        'order', OLD."order",
        'back_type', OLD.back_type,
        'created_at', OLD.created_at,
        'updated_at', OLD.updated_at,
        'due', OLD.due,
        'stability', OLD.stability,
        'stability_fast', OLD.stability_fast,
        'difficulty', OLD.difficulty,
        'desired_retention', OLD.desired_retention,
        'special_state', OLD.special_state,
        'state', OLD.state,
        'custom_data', json(OLD.custom_data),
        'tags', (SELECT json_group_array(tag_id) FROM
            (SELECT tag_id FROM card_tag WHERE card_id = OLD.id ORDER BY tag_id))
    ));
    UPDATE card SET usn = -1 WHERE id = NEW.id;
END;

CREATE TRIGGER sync_note_tag_insert BEFORE INSERT ON note_tag
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'note', s.id, s.snapshot FROM sync_note_snapshot s JOIN note n ON n.id = s.id
    WHERE s.id = NEW.note_id AND n.usn != -1;
    UPDATE note SET usn = -1 WHERE id = NEW.note_id AND usn != -1;
END;

CREATE TRIGGER sync_note_tag_delete BEFORE DELETE ON note_tag
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    AND EXISTS (SELECT 1 FROM tag WHERE id = OLD.tag_id)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'note', s.id, s.snapshot FROM sync_note_snapshot s JOIN note n ON n.id = s.id
    WHERE s.id = OLD.note_id AND n.usn != -1;
    UPDATE note SET usn = -1 WHERE id = OLD.note_id AND usn != -1;
END;

CREATE TRIGGER sync_note_keyword_insert BEFORE INSERT ON note_keyword
WHEN NEW.embedded = 0 AND NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'note', s.id, s.snapshot FROM sync_note_snapshot s JOIN note n ON n.id = s.id
    WHERE s.id = NEW.note_id AND n.usn != -1;
    UPDATE note SET usn = -1 WHERE id = NEW.note_id AND usn != -1;
END;

CREATE TRIGGER sync_note_keyword_delete BEFORE DELETE ON note_keyword
WHEN OLD.embedded = 0 AND NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'note', s.id, s.snapshot FROM sync_note_snapshot s JOIN note n ON n.id = s.id
    WHERE s.id = OLD.note_id AND n.usn != -1;
    UPDATE note SET usn = -1 WHERE id = OLD.note_id AND usn != -1;
END;

CREATE TRIGGER sync_card_tag_insert BEFORE INSERT ON card_tag
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'card', s.id, s.snapshot FROM sync_card_snapshot s JOIN card c ON c.id = s.id
    WHERE s.id = NEW.card_id AND c.usn != -1;
    UPDATE card SET usn = -1 WHERE id = NEW.card_id AND usn != -1;
END;

CREATE TRIGGER sync_card_tag_delete BEFORE DELETE ON card_tag
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
    AND EXISTS (SELECT 1 FROM tag WHERE id = OLD.tag_id)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'card', s.id, s.snapshot FROM sync_card_snapshot s JOIN card c ON c.id = s.id
    WHERE s.id = OLD.card_id AND c.usn != -1;
    UPDATE card SET usn = -1 WHERE id = OLD.card_id AND usn != -1;
END;

CREATE TRIGGER sync_parser_before_delete BEFORE DELETE ON parser
WHEN OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('parser', OLD.id, json_object('id', OLD.id, 'name', OLD.name));
END;

CREATE TRIGGER sync_note_before_delete BEFORE DELETE ON note
WHEN OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'note', id, snapshot FROM sync_note_snapshot WHERE id = OLD.id;
END;

CREATE TRIGGER sync_card_before_delete BEFORE DELETE ON card
WHEN OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    SELECT 'card', id, snapshot FROM sync_card_snapshot WHERE id = OLD.id;
END;

CREATE TRIGGER sync_parser_delete AFTER DELETE ON parser
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR REPLACE INTO grave (oid, kind, usn) VALUES (OLD.id, 'parser', -1);
END;

CREATE TRIGGER sync_tag_delete AFTER DELETE ON tag
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR REPLACE INTO grave (oid, kind, usn) VALUES (OLD.id, 'tag', -1);
END;

CREATE TRIGGER sync_note_delete AFTER DELETE ON note
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR REPLACE INTO grave (oid, kind, usn) VALUES (OLD.id, 'note', -1);
END;

CREATE TRIGGER sync_card_delete AFTER DELETE ON card
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR REPLACE INTO grave (oid, kind, usn) VALUES (OLD.id, 'card', -1);
END;

CREATE TRIGGER sync_parser_insert AFTER INSERT ON parser
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    DELETE FROM grave WHERE kind = 'parser' AND oid = NEW.id;
END;

CREATE TRIGGER sync_tag_insert AFTER INSERT ON tag
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    DELETE FROM grave WHERE kind = 'tag' AND oid = NEW.id;
END;

CREATE TRIGGER sync_note_insert AFTER INSERT ON note
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    DELETE FROM grave WHERE kind = 'note' AND oid = NEW.id;
END;

CREATE TRIGGER sync_card_insert AFTER INSERT ON card
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    DELETE FROM grave WHERE kind = 'card' AND oid = NEW.id;
END;

CREATE TRIGGER sync_tag_update AFTER UPDATE ON tag
WHEN NEW.usn = OLD.usn AND OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.name IS NOT NEW.name OR OLD.description IS NOT NEW.description
        OR OLD.query IS NOT NEW.query OR OLD.auto_delete IS NOT NEW.auto_delete
        OR OLD.reschedule IS NOT NEW.reschedule)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('tag', OLD.id, json_object(
        'id', OLD.id,
        'name', OLD.name,
        'description', OLD.description,
        'query', OLD.query,
        'auto_delete', OLD.auto_delete,
        'reschedule', OLD.reschedule,
        'created_at', OLD.created_at,
        'updated_at', OLD.updated_at
    ));
    UPDATE tag SET usn = -1 WHERE id = NEW.id;
END;

CREATE TRIGGER sync_tag_before_delete BEFORE DELETE ON tag
WHEN OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('tag', OLD.id, json_object(
        'id', OLD.id,
        'name', OLD.name,
        'description', OLD.description,
        'query', OLD.query,
        'auto_delete', OLD.auto_delete,
        'reschedule', OLD.reschedule,
        'created_at', OLD.created_at,
        'updated_at', OLD.updated_at
    ));
END;

CREATE TRIGGER review_log_previous_interval_insert AFTER INSERT ON review_log
WHEN NEW.kind = 0 AND NEW.card_id IS NOT NULL
BEGIN
    UPDATE review_log SET previous_interval = (
        SELECT p.scheduled_time FROM review_log p
        WHERE p.card_id = review_log.card_id AND p.kind = 0
          AND p.reviewed_at <= review_log.reviewed_at
          AND (p.reviewed_at < review_log.reviewed_at OR p.id < review_log.id)
        ORDER BY p.reviewed_at DESC, p.id DESC LIMIT 1)
    WHERE id IN (NEW.id, (
        SELECT n.id FROM review_log n
        WHERE n.card_id = NEW.card_id AND n.kind = 0
          AND n.reviewed_at >= NEW.reviewed_at
          AND (n.reviewed_at > NEW.reviewed_at OR n.id > NEW.id)
        ORDER BY n.reviewed_at, n.id LIMIT 1));
END;

CREATE TRIGGER review_log_previous_interval_delete AFTER DELETE ON review_log
WHEN OLD.kind = 0 AND OLD.card_id IS NOT NULL
BEGIN
    UPDATE review_log SET previous_interval = OLD.previous_interval
    WHERE id = (
        SELECT n.id FROM review_log n
        WHERE n.card_id = OLD.card_id AND n.kind = 0
          AND n.reviewed_at >= OLD.reviewed_at
          AND (n.reviewed_at > OLD.reviewed_at OR n.id > OLD.id)
        ORDER BY n.reviewed_at, n.id LIMIT 1);
END;

CREATE TRIGGER review_log_previous_interval_update
AFTER UPDATE OF card_id, reviewed_at, kind, scheduled_time ON review_log
BEGIN
    UPDATE review_log SET previous_interval = CASE
        WHEN review_log.kind = 0 AND review_log.card_id IS NOT NULL THEN (
            SELECT p.scheduled_time FROM review_log p
            WHERE p.card_id = review_log.card_id AND p.kind = 0
              AND p.reviewed_at <= review_log.reviewed_at
              AND (p.reviewed_at < review_log.reviewed_at OR p.id < review_log.id)
            ORDER BY p.reviewed_at DESC, p.id DESC LIMIT 1)
        END
    WHERE id IN (NEW.id, (
        SELECT n.id FROM review_log n
        WHERE n.card_id = OLD.card_id AND n.kind = 0 AND n.id != NEW.id
          AND n.reviewed_at >= OLD.reviewed_at
          AND (n.reviewed_at > OLD.reviewed_at OR n.id > OLD.id)
        ORDER BY n.reviewed_at, n.id LIMIT 1), (
        SELECT n.id FROM review_log n
        WHERE n.card_id = NEW.card_id AND n.kind = 0 AND n.id != NEW.id
          AND n.reviewed_at >= NEW.reviewed_at
          AND (n.reviewed_at > NEW.reviewed_at OR n.id > NEW.id)
        ORDER BY n.reviewed_at, n.id LIMIT 1));
END;

CREATE TRIGGER note_fts_insert AFTER INSERT ON note
BEGIN
    INSERT INTO note_fts (rowid, data, keywords)
    VALUES (NEW.id, NEW.data, COALESCE((SELECT group_concat(nk.keyword, ' ') FROM note_keyword nk WHERE nk.note_id = NEW.id), ''));
END;

CREATE TRIGGER note_fts_update AFTER UPDATE OF id, data ON note
BEGIN
    DELETE FROM note_fts WHERE rowid = OLD.id;
    INSERT INTO note_fts (rowid, data, keywords)
    VALUES (NEW.id, NEW.data, COALESCE((SELECT group_concat(nk.keyword, ' ') FROM note_keyword nk WHERE nk.note_id = NEW.id), ''));
END;

CREATE TRIGGER note_fts_delete AFTER DELETE ON note
BEGIN
    DELETE FROM note_fts WHERE rowid = OLD.id;
END;

CREATE TRIGGER note_fts_keyword_insert AFTER INSERT ON note_keyword
BEGIN
    DELETE FROM note_fts WHERE rowid = NEW.note_id;
    INSERT INTO note_fts (rowid, data, keywords)
    SELECT n.id, n.data, COALESCE((SELECT group_concat(nk.keyword, ' ') FROM note_keyword nk WHERE nk.note_id = n.id), '')
    FROM note n WHERE n.id = NEW.note_id;
END;

CREATE TRIGGER note_fts_keyword_delete AFTER DELETE ON note_keyword
BEGIN
    DELETE FROM note_fts WHERE rowid = OLD.note_id;
    INSERT INTO note_fts (rowid, data, keywords)
    SELECT n.id, n.data, COALESCE((SELECT group_concat(nk.keyword, ' ') FROM note_keyword nk WHERE nk.note_id = n.id), '')
    FROM note n WHERE n.id = OLD.note_id;
END;

CREATE TRIGGER note_fts_keyword_update AFTER UPDATE OF note_id, keyword ON note_keyword
BEGIN
    DELETE FROM note_fts WHERE rowid IN (OLD.note_id, NEW.note_id);
    INSERT INTO note_fts (rowid, data, keywords)
    SELECT n.id, n.data, COALESCE((SELECT group_concat(nk.keyword, ' ') FROM note_keyword nk WHERE nk.note_id = n.id), '')
    FROM note n WHERE n.id IN (OLD.note_id, NEW.note_id);
END;

CREATE TRIGGER sync_saved_search_update AFTER UPDATE ON saved_search
WHEN NEW.usn = OLD.usn AND OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.name IS NOT NEW.name OR OLD.query IS NOT NEW.query)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('saved_search', OLD.id, json_object(
        'id', OLD.id,
        'name', OLD.name,
        'query', OLD.query,
        'created_at', OLD.created_at,
        'updated_at', OLD.updated_at
    ));
    UPDATE saved_search SET usn = -1 WHERE id = NEW.id;
END;

CREATE TRIGGER sync_saved_search_before_delete BEFORE DELETE ON saved_search
WHEN OLD.usn != -1 AND NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR IGNORE INTO sync_base (kind, oid, snapshot)
    VALUES ('saved_search', OLD.id, json_object(
        'id', OLD.id,
        'name', OLD.name,
        'query', OLD.query,
        'created_at', OLD.created_at,
        'updated_at', OLD.updated_at
    ));
END;

CREATE TRIGGER sync_saved_search_delete AFTER DELETE ON saved_search
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR REPLACE INTO grave (oid, kind, usn) VALUES (OLD.id, 'saved_search', -1);
END;

CREATE TRIGGER sync_saved_search_insert AFTER INSERT ON saved_search
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    DELETE FROM grave WHERE kind = 'saved_search' AND oid = NEW.id;
END;

CREATE TRIGGER sync_review_log_delete AFTER DELETE ON review_log
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT OR REPLACE INTO grave (oid, kind, usn) VALUES (OLD.id, 'review_log', -1);
END;

CREATE TRIGGER sync_review_log_insert AFTER INSERT ON review_log
WHEN NOT EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    DELETE FROM grave WHERE kind = 'review_log' AND oid = NEW.id;
END;

CREATE TRIGGER sync_received_parser_insert AFTER INSERT ON parser
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('parser', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_parser_update AFTER UPDATE ON parser
WHEN EXISTS (SELECT 1 FROM sync_applying)
    AND OLD.name IS NOT NEW.name
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('parser', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_parser_delete AFTER DELETE ON parser
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('parser', OLD.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_tag_insert AFTER INSERT ON tag
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('tag', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_tag_update AFTER UPDATE ON tag
WHEN EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.name IS NOT NEW.name OR OLD.description IS NOT NEW.description
        OR OLD.query IS NOT NEW.query OR OLD.auto_delete IS NOT NEW.auto_delete
        OR OLD.reschedule IS NOT NEW.reschedule)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('tag', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_tag_delete AFTER DELETE ON tag
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('tag', OLD.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_note_insert AFTER INSERT ON note
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_note_update AFTER UPDATE ON note
WHEN EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.data IS NOT NEW.data OR OLD.custom_data IS NOT NEW.custom_data
        OR OLD.parser_id IS NOT NEW.parser_id OR OLD.created_at IS NOT NEW.created_at)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_note_delete AFTER DELETE ON note
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', OLD.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_card_insert AFTER INSERT ON card
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('card', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', NEW.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_card_update AFTER UPDATE ON card
WHEN EXISTS (SELECT 1 FROM sync_applying)
    AND (OLD.note_id IS NOT NEW.note_id OR OLD."order" IS NOT NEW."order"
        OR OLD.back_type IS NOT NEW.back_type OR OLD.due IS NOT NEW.due
        OR OLD.stability IS NOT NEW.stability OR OLD.stability_fast IS NOT NEW.stability_fast
        OR OLD.difficulty IS NOT NEW.difficulty
        OR OLD.desired_retention IS NOT NEW.desired_retention
        OR OLD.special_state IS NOT NEW.special_state OR OLD.state IS NOT NEW.state
        OR OLD.custom_data IS NOT NEW.custom_data OR OLD.created_at IS NOT NEW.created_at)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('card', NEW.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', NEW.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', OLD.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_card_delete AFTER DELETE ON card
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('card', OLD.id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', OLD.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_note_tag_insert AFTER INSERT ON note_tag
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', NEW.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_note_tag_delete AFTER DELETE ON note_tag
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', OLD.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_card_tag_insert AFTER INSERT ON card_tag
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('card', NEW.card_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
    -- None when the card is being deleted, which counts for its note itself
    INSERT INTO sync_received (kind, oid, event_id)
    SELECT 'note', note_id, (SELECT COALESCE(MAX(id), 0) FROM event) FROM card WHERE id = NEW.card_id
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_card_tag_delete AFTER DELETE ON card_tag
WHEN EXISTS (SELECT 1 FROM sync_applying)
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('card', OLD.card_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
    -- None when the card is being deleted, which counts for its note itself
    INSERT INTO sync_received (kind, oid, event_id)
    SELECT 'note', note_id, (SELECT COALESCE(MAX(id), 0) FROM event) FROM card WHERE id = OLD.card_id
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_note_keyword_insert AFTER INSERT ON note_keyword
WHEN EXISTS (SELECT 1 FROM sync_applying) AND NEW.embedded = 0
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', NEW.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER sync_received_note_keyword_delete AFTER DELETE ON note_keyword
WHEN EXISTS (SELECT 1 FROM sync_applying) AND OLD.embedded = 0
BEGIN
    INSERT INTO sync_received (kind, oid, event_id)
    VALUES ('note', OLD.note_id, (SELECT COALESCE(MAX(id), 0) FROM event))
    ON CONFLICT (kind, oid) DO UPDATE SET event_id = excluded.event_id;
END;

CREATE TRIGGER keyword_version_insert AFTER INSERT ON note_keyword
BEGIN
    UPDATE keyword_version SET token = randomblob(16);
END;

CREATE TRIGGER keyword_version_delete AFTER DELETE ON note_keyword
BEGIN
    UPDATE keyword_version SET token = randomblob(16);
END;

CREATE TRIGGER keyword_version_update AFTER UPDATE OF note_id, keyword ON note_keyword
BEGIN
    UPDATE keyword_version SET token = randomblob(16);
END;

CREATE TRIGGER study_day_insert AFTER INSERT ON review_log
WHEN NEW.card_id IS NOT NULL AND NEW.kind = 0 AND NEW.reviewed_at >= (SELECT since FROM study_day_cache)
BEGIN
    INSERT INTO study_day_dirty VALUES (NEW.card_id, NEW.reviewed_at) ON CONFLICT DO NOTHING;
END;

CREATE TRIGGER study_day_delete AFTER DELETE ON review_log
WHEN OLD.card_id IS NOT NULL AND OLD.kind = 0 AND OLD.reviewed_at >= (SELECT since FROM study_day_cache)
BEGIN
    INSERT INTO study_day_dirty VALUES (OLD.card_id, OLD.reviewed_at) ON CONFLICT DO NOTHING;
END;

CREATE TRIGGER study_day_update
AFTER UPDATE OF card_id, reviewed_at, kind, previous_state, recall_duration, rate_duration, id ON review_log
BEGIN
    INSERT INTO study_day_dirty SELECT OLD.card_id, OLD.reviewed_at WHERE OLD.card_id IS NOT NULL AND OLD.kind = 0
        AND OLD.reviewed_at >= (SELECT since FROM study_day_cache) ON CONFLICT DO NOTHING;
    INSERT INTO study_day_dirty SELECT NEW.card_id, NEW.reviewed_at WHERE NEW.card_id IS NOT NULL AND NEW.kind = 0
        AND NEW.reviewed_at >= (SELECT since FROM study_day_cache) ON CONFLICT DO NOTHING;
END;

-- Refuses to commit if a row refers to one that doesn't exist. The failed constraint names it.
CREATE TEMP TABLE strict_schema_guard (a_foreign_key_is_violated INTEGER NOT NULL);
INSERT INTO strict_schema_guard SELECT NULL FROM pragma_foreign_key_check LIMIT 1;
DROP TABLE strict_schema_guard;

COMMIT;

PRAGMA foreign_keys = ON;
