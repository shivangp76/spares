-- The largest id each replica has ever handed out, per table. Deletes never lower it, so ids are
-- never handed out twice, even after the newest rows are deleted (see `ids::allocate_note_ids`).
-- Keyed by replica because a full sync copies the other replica's rows too.
CREATE TABLE IF NOT EXISTS id_counter (
    replica INTEGER NOT NULL,
    tbl TEXT NOT NULL,
    last INTEGER NOT NULL,
    PRIMARY KEY (replica, tbl)
);
