-- Study actions a client named with an id (`SubmitStudyActionRequest::action_id`), so that sending
-- one again, such as after its response was lost, returns what the first attempt did rather than
-- rating or burying the card twice. Each collection has its own table, so ids are scoped to it.
-- `request` is the action as JSON, to tell a retry from a different action reusing the id, and
-- `response` what it returned. Rows older than 30 days are deleted as actions are submitted.
CREATE TABLE IF NOT EXISTS study_action (
    id TEXT PRIMARY KEY NOT NULL,
    request TEXT NOT NULL,
    response TEXT NOT NULL,
    created_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS study_action_created_at ON study_action (created_at);
