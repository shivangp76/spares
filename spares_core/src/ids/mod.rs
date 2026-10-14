//! Note ids: short lowercase letter strings (`gir`, `giza`) that are unique across replicas without
//! coordination, sort in creation order, and keep notes created together next to each other. See
//! section 4 of `docs/sync-plan.md` for the reasoning.
//!
//! An id is stored as an `i64`: its letters read as a base-27 number left-aligned to
//! [`MAX_LETTERS`] places (`a` = 1 … `z` = 26, an empty place = 0). Comparing the integers
//! therefore compares the strings, whatever their lengths, and the database schema and the JSON
//! wire format keep using integers.
//!
//! Ids are handed out by [`allocate_note_ids`]:
//! - **Blocks** are consecutive letter runs `aa`, `ab`, …, `sz`, `taa`, …, whose first letter decides
//!   their length (see [`block_len`]).
//! - The laptop (replica 0) writes `<block><slot>`, with slots `a`–`y`.
//! - The server (replica 1) writes `<block>z<tail>`, after the laptop's notes in the newest block it
//!   has seen.
//!
//! No id is the start of another, so sorting the strings sorts by block, then slot, then tail.

pub mod migrate;

use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;
use sqlx::SqliteConnection;
use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::config::Replica;
use crate::config::get_replica;

/// The longest id that can be packed.
pub const MAX_LETTERS: usize = 8;
const RADIX: i64 = 27;
/// The packed value of `a`, the smallest id. Anything below it is a numeric id from before
/// `spares migrate ids`.
pub const MIN_PACKED: i64 = RADIX.pow(MAX_LETTERS as u32 - 1);
/// The packed value of `zzzzzzzz`.
pub const MAX_PACKED: i64 = RADIX.pow(MAX_LETTERS as u32) - 1;
/// Letters a laptop slot or the first letter of a server tail can take: `a`–`y`. `z` marks the
/// server's part of a block.
const SLOTS: u8 = 25;
const Z: u8 = 25;

/// File mapping old numeric note ids to their letter ids, written to the data directory by
/// `spares migrate ids`.
pub const ID_MAP_FILE_NAME: &str = "id-map.tsv";

/// `app_state` key set once the database's note ids are letter ids.
pub const NOTE_IDS_MIGRATED_KEY: &str = "note_ids_migrated";

/// A note id. Shown as letters ([`NoteId::letters`]), stored and sent as the packed integer.
#[derive(Clone, Copy, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, sqlx::Type)]
#[serde(transparent)]
#[sqlx(transparent)]
pub struct NoteId(i64);

impl NoteId {
    /// The note of a [`crate::model::Card`] that belongs to no note yet, such as the cards a
    /// scheduler replays review logs into. It is `0`, which is not a valid id, so it can never name
    /// a note. Deliberately not `Default`, so it is only used where it is spelled out.
    pub const UNSET: Self = Self(0);

    /// Wraps a packed value as read from the database or the API, without checking it.
    pub const fn from_packed(packed: i64) -> Self {
        Self(packed)
    }

    pub const fn packed(self) -> i64 {
        self.0
    }

    /// The id's letters, or `None` if the packed value is not a valid id (for example a numeric id
    /// from before the migration).
    pub fn try_letters(self) -> Option<String> {
        unpack(self.0).map(|letters| letters.into_iter().map(|l| char::from(b'a' + l)).collect())
    }

    /// The id's letters. An invalid packed value is shown as `#<number>`, which no file name or
    /// search produced by spares contains.
    pub fn letters(self) -> String {
        self.try_letters().unwrap_or_else(|| format!("#{}", self.0))
    }

    /// Parses 1 to [`MAX_LETTERS`] letters, in any case.
    pub fn from_letters(s: &str) -> Result<Self, ParseNoteIdError> {
        let s = s.trim();
        if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ParseNoteIdError(format!(
                "`{s}` is an old numeric note id. Note ids are letters now; look it up in \
                 `{ID_MAP_FILE_NAME}` in the data directory."
            )));
        }
        if s.is_empty() || s.len() > MAX_LETTERS || !s.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err(ParseNoteIdError(format!(
                "`{s}` is not a note id: expected 1 to {MAX_LETTERS} letters"
            )));
        }
        let letters: Vec<u8> = s.bytes().map(|b| b.to_ascii_lowercase() - b'a').collect();
        Ok(Self(pack(&letters).expect("length was checked")))
    }

    fn from_letter_indices(letters: &[u8]) -> Result<Self, Error> {
        pack(letters).map(Self).ok_or_else(|| {
            note_error(format!(
                "Ran out of note ids: the next one would be longer than {MAX_LETTERS} letters"
            ))
        })
    }

    fn letter_indices(self) -> Option<Vec<u8>> {
        unpack(self.0)
    }
}

/// The id's letters (see [`NoteId::letters`]).
impl fmt::Display for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.letters())
    }
}

impl fmt::Debug for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NoteId({})", self.letters())
    }
}

impl FromStr for NoteId {
    type Err = ParseNoteIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_letters(s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseNoteIdError(String);

impl fmt::Display for ParseNoteIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseNoteIdError {}

/// Packs letter indices (`a` = 0) into the integer form, or `None` for an empty or too long id.
fn pack(letters: &[u8]) -> Option<i64> {
    if letters.is_empty() || letters.len() > MAX_LETTERS {
        return None;
    }
    let mut packed = 0;
    for place in 0..MAX_LETTERS {
        packed = packed * RADIX + letters.get(place).map_or(0, |&l| i64::from(l) + 1);
    }
    Some(packed)
}

/// The letter indices of a packed id, or `None` if it is not one.
fn unpack(packed: i64) -> Option<Vec<u8>> {
    if !(MIN_PACKED..=MAX_PACKED).contains(&packed) {
        return None;
    }
    let mut places = [0u8; MAX_LETTERS];
    let mut rest = packed;
    for place in places.iter_mut().rev() {
        *place = u8::try_from(rest % RADIX).expect("a base-27 digit of a positive number");
        rest /= RADIX;
    }
    let len = places.iter().position(|&p| p == 0).unwrap_or(MAX_LETTERS);
    // An empty place must not be followed by a letter.
    if places[len..].iter().any(|&p| p != 0) {
        return None;
    }
    Some(places[..len].iter().map(|&p| p - 1).collect())
}

/// Length of a block whose first letter is `first`: 2 for `a`–`s`, 3 for `t`–`x`, 4 for `y` and 5
/// for `z`. Blocks grow instead of running out.
fn block_len(first: u8) -> usize {
    match first {
        0..=18 => 2,
        19..=23 => 3,
        24 => 4,
        _ => 5,
    }
}

fn first_block() -> Vec<u8> {
    vec![0, 0]
}

/// The block after `block`, moving to the next tier's length when the first letter changes tier.
fn next_block(block: &[u8]) -> Result<Vec<u8>, Error> {
    let mut next = block.to_vec();
    for i in (0..next.len()).rev() {
        if next[i] < Z {
            next[i] += 1;
            if i == 0 {
                next.resize(block_len(next[0]), 0);
            }
            return Ok(next);
        }
        next[i] = 0;
    }
    Err(note_error(
        "Ran out of note ids: every block has been used".to_string(),
    ))
}

/// Splits an id into its block and the rest. A shorter id is padded with `a`, which gives the
/// smallest block that sorts after it.
fn split_block(id: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let len = block_len(id[0]);
    if id.len() < len {
        let mut block = id.to_vec();
        block.resize(len, 0);
        return (block, Vec::new());
    }
    (id[..len].to_vec(), id[len..].to_vec())
}

/// `n` laptop ids after `newest`, the newest note this replica has created or seen.
fn laptop_ids_after(newest: Option<&[u8]>, n: usize) -> Result<Vec<Vec<u8>>, Error> {
    let (mut block, mut slot) = match newest {
        None => (first_block(), 0),
        Some(id) => {
            let (block, rest) = split_block(id);
            match rest.first() {
                // A block on its own (or a shorter id padded into one) sorts before its slots.
                None => (block, 0),
                Some(&slot) if slot + 1 < SLOTS => (block, slot + 1),
                // The block is full, or the newest note is the server's.
                Some(_) => (next_block(&block)?, 0),
            }
        }
    };
    // A call that does not fit in what is left of the block starts at the next one, so no server
    // note can sort inside it.
    if slot > 0 && n > usize::from(SLOTS - slot) {
        block = next_block(&block)?;
        slot = 0;
    }
    let mut ids = Vec::with_capacity(n);
    for _ in 0..n {
        if slot == SLOTS {
            block = next_block(&block)?;
            slot = 0;
        }
        let mut id = block.clone();
        id.push(slot);
        ids.push(id);
        slot += 1;
    }
    Ok(ids)
}

/// The first server tail: `a`.
fn first_tail() -> Vec<u8> {
    vec![0]
}

/// The smallest tail after `tail`. Tails of level `k` are `k` letters `z`, a letter `a`–`y`, then
/// `k` letters `a`–`z`, so no tail is the start of another and each level sorts after the previous
/// one: `a`…`y`, `zaa`…`zyz`, `zzaaa`, ….
fn next_tail(tail: &[u8]) -> Vec<u8> {
    let level = tail.iter().take_while(|&&l| l == Z).count();
    if level == tail.len() {
        // Only `z`s (or nothing): the start of this level sorts after them.
        let mut next = vec![Z; level];
        next.extend(std::iter::repeat_n(0, level + 1));
        return next;
    }
    let mut next = tail[..=level].to_vec();
    let suffix = &tail[level + 1..];
    if suffix.len() < level {
        // Shorter than a tail of this level: padding it gives one that sorts after it.
        next.extend_from_slice(suffix);
        next.resize(2 * level + 1, 0);
        return next;
    }
    next.extend_from_slice(&suffix[..level]);
    for i in (level..next.len()).rev() {
        let max = if i == level { SLOTS - 1 } else { Z };
        if next[i] < max {
            next[i] += 1;
            return next;
        }
        next[i] = 0;
    }
    // The level is full.
    let mut next = vec![Z; level + 1];
    next.extend(std::iter::repeat_n(0, level + 2));
    next
}

/// `n` server ids after `newest`, in the server's part of the newest block it has seen.
fn server_ids_after(newest: Option<&[u8]>, n: usize) -> Vec<Vec<u8>> {
    let (block, mut tail) = match newest {
        None => (first_block(), None),
        Some(id) => {
            let (block, rest) = split_block(id);
            match rest.split_first() {
                Some((&Z, tail)) => (block, Some(tail.to_vec())),
                _ => (block, None),
            }
        }
    };
    let mut ids = Vec::with_capacity(n);
    for _ in 0..n {
        let next = tail.as_deref().map_or_else(first_tail, next_tail);
        let mut id = block.clone();
        id.push(Z);
        id.extend_from_slice(&next);
        ids.push(id);
        tail = Some(next);
    }
    ids
}

/// `n` ids `replica` would hand out after `newest`. Pure, so it can be tested without a database.
fn ids_after(replica: Replica, newest: Option<NoteId>, n: usize) -> Result<Vec<NoteId>, Error> {
    let newest = newest
        .map(|id| {
            id.letter_indices().ok_or_else(|| {
                note_error(format!(
                    "Cannot allocate note ids after {}, which is not a letter id. Run `spares \
                     migrate ids` first.",
                    id.packed()
                ))
            })
        })
        .transpose()?;
    let ids = match replica.number() {
        0 => laptop_ids_after(newest.as_deref(), n)?,
        1 => server_ids_after(newest.as_deref(), n),
        number => {
            return Err(note_error(format!(
                "Replica {number} cannot create notes yet: only replicas 0 (laptop) and 1 (server) \
                 have note id rules. See \"A third replica\" in docs/sync-plan.md."
            )));
        }
    };
    ids.iter()
        .map(|id| NoteId::from_letter_indices(id))
        .collect()
}

/// Hands out `n` new note ids for this replica, in order, as one run. Give it all of a call's
/// notes at once, so an import gets ids next to each other.
///
/// The ids are recorded in `id_counter` before they are used, so they are never handed out again,
/// even if the notes are deleted or never inserted.
pub async fn allocate_note_ids(db: &SqlitePool, n: usize) -> Result<Vec<NoteId>, Error> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut tx = db.begin().await.map_err(|e| Error::Sqlx { source: e })?;
    let ids = allocate_note_ids_on(&mut tx, get_replica()?, n).await?;
    tx.commit().await.map_err(|e| Error::Sqlx { source: e })?;
    Ok(ids)
}

/// [`allocate_note_ids`] inside the caller's transaction.
pub(crate) async fn allocate_note_ids_on(
    conn: &mut SqliteConnection,
    replica: Replica,
    n: usize,
) -> Result<Vec<NoteId>, Error> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let replica_number = i64::from(replica.number());
    // Writing first takes SQLite's write lock before the maximum is read, so two concurrent
    // allocations cannot both read the same maximum.
    sqlx::query(
        r"INSERT INTO id_counter (replica, tbl, last) VALUES (?, 'note', 0)
          ON CONFLICT (replica, tbl) DO UPDATE SET last = last",
    )
    .bind(replica_number)
    .execute(&mut *conn)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let newest: Option<i64> = sqlx::query_scalar(
        r"SELECT MAX(m) FROM (
            SELECT last AS m FROM id_counter WHERE replica = ? AND tbl = 'note' AND last > 0
            UNION ALL SELECT MAX(id) FROM note
          )",
    )
    .bind(replica_number)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let ids = ids_after(replica, newest.map(NoteId::from_packed), n)?;
    let last = ids.last().expect("n > 0").packed();
    sqlx::query(r"UPDATE id_counter SET last = ? WHERE replica = ? AND tbl = 'note'")
        .bind(last)
        .bind(replica_number)
        .execute(&mut *conn)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(ids)
}

/// The step between the ids one replica hands out for [`IdTable`]s. Each replica takes the ids
/// congruent to its number modulo this, so replicas never collide. It is fixed rather than the
/// current number of replicas, so adding a replica never changes the rule for the others.
pub const ID_STEP: i64 = 8;

/// A table whose ids are handed out by [`next_ids`]. Notes use [`allocate_note_ids`] instead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdTable {
    Card,
    ReviewLog,
    Tag,
    Parser,
}

impl IdTable {
    pub const ALL: [Self; 4] = [Self::Card, Self::ReviewLog, Self::Tag, Self::Parser];

    /// The table's name, which is also its `id_counter.tbl` key.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Card => "card",
            Self::ReviewLog => "review_log",
            Self::Tag => "tag",
            Self::Parser => "parser",
        }
    }
}

/// Hands out `n` new ids for `table`, in increasing order: the smallest ids above both the
/// high-water marks (this replica's, and any other's a full sync brought) and the table's largest
/// id that are congruent to the replica number modulo [`ID_STEP`]. Rows created before sync keep
/// their ids, which are all below the new ones.
///
/// It is one statement, so it needs no surrounding transaction to be safe against a concurrent
/// allocation, and it can run inside the caller's.
pub async fn next_ids(
    conn: &mut SqliteConnection,
    table: IdTable,
    n: usize,
) -> Result<Vec<i64>, Error> {
    next_ids_for(conn, get_replica()?, table, n).await
}

/// [`next_ids`] for one id.
pub async fn next_id(conn: &mut SqliteConnection, table: IdTable) -> Result<i64, Error> {
    Ok(next_ids(conn, table, 1).await?[0])
}

/// [`next_ids`] on a connection taken from `db`.
pub async fn next_ids_in(db: &SqlitePool, table: IdTable, n: usize) -> Result<Vec<i64>, Error> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut conn = db.acquire().await.map_err(|e| Error::Sqlx { source: e })?;
    next_ids(&mut conn, table, n).await
}

/// [`next_id`] on a connection taken from `db`.
pub async fn next_id_in(db: &SqlitePool, table: IdTable) -> Result<i64, Error> {
    Ok(next_ids_in(db, table, 1).await?[0])
}

pub(crate) async fn next_ids_for(
    conn: &mut SqliteConnection,
    replica: Replica,
    table: IdTable,
    n: usize,
) -> Result<Vec<i64>, Error> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let replica_number = i64::from(replica.number());
    let count = i64::try_from(n).expect("n fits in i64");
    // `base` is the newest id this replica has handed out or seen. `first` is the smallest id
    // above it in this replica's residue class.
    let query = format!(
        r"INSERT INTO id_counter (replica, tbl, last)
          SELECT ?1, ?2, first + (?3 - 1) * ?4 FROM (
            SELECT base - base % ?4 + ?1 + CASE WHEN base % ?4 >= ?1 THEN ?4 ELSE 0 END AS first
            FROM (SELECT MAX(
              COALESCE((SELECT MAX(last) FROM id_counter WHERE tbl = ?2), 0),
              COALESCE((SELECT MAX(id) FROM {table}), 0)
            ) AS base)
          ) WHERE true
          ON CONFLICT (replica, tbl) DO UPDATE SET last = excluded.last
          RETURNING last",
        table = table.name()
    );
    let last: i64 = sqlx::query_scalar(&query)
        .bind(replica_number)
        .bind(table.name())
        .bind(count)
        .bind(ID_STEP)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok((0..count)
        .map(|i| last - (count - 1 - i) * ID_STEP)
        .collect())
}

/// Whether the database's note ids are letter ids. A database with no notes is marked as migrated
/// here, since it has nothing to migrate.
pub async fn ensure_note_ids_migrated(db: &SqlitePool) -> Result<(), Error> {
    let flag: Option<String> = sqlx::query_scalar("SELECT value FROM app_state WHERE key = ?")
        .bind(NOTE_IDS_MIGRATED_KEY)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let (count, numeric): (i64, i64) =
        sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(id < ?), 0) FROM note")
            .bind(MIN_PACKED)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    if flag.is_none() && count == 0 {
        sqlx::query("INSERT OR IGNORE INTO app_state (key, value) VALUES (?, 'true')")
            .bind(NOTE_IDS_MIGRATED_KEY)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        return Ok(());
    }
    if flag.is_none() || numeric > 0 {
        return Err(note_error(format!(
            "The database still has numeric note ids ({numeric} of {count} notes). Stop the \
             server and run `spares migrate ids` (try `--dry-run` first)."
        )));
    }
    Ok(())
}

/// Inserts a bare note with a newly allocated id, for tests that need a note row but not
/// everything [`crate::api::note::create_notes`] does.
#[cfg(test)]
pub(crate) async fn insert_test_note(db: &SqlitePool, data: &str, parser_id: i64) -> NoteId {
    let id = allocate_note_ids(db, 1).await.unwrap()[0];
    sqlx::query("INSERT INTO note (id, data, custom_data, parser_id) VALUES (?, ?, '{}', ?)")
        .bind(id)
        .bind(data)
        .bind(parser_id)
        .execute(db)
        .await
        .unwrap();
    id
}

fn note_error(description: String) -> Error {
    Error::Library(LibraryError::Note(crate::NoteErrorKind::Other {
        description,
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use rand::RngExt;
    use sqlx::SqlitePool;

    use super::*;

    const LAPTOP: Replica = Replica::new_unchecked(0);
    const SERVER: Replica = Replica::new_unchecked(1);

    fn id(s: &str) -> NoteId {
        s.parse().unwrap()
    }

    fn letters(ids: &[NoteId]) -> Vec<String> {
        ids.iter().map(|id| id.letters()).collect()
    }

    #[test]
    fn test_pack_round_trip() {
        for s in [
            "a", "z", "gir", "giza", "gizzaa", "zzzzzzzz", "abcdefgh", "taaa",
        ] {
            assert_eq!(id(s).letters(), s);
        }
        assert_eq!(id("gir").packed(), 76_967_537_148);
        assert_eq!(id("a").packed(), MIN_PACKED);
        assert_eq!(id("zzzzzzzz").packed(), MAX_PACKED);
        let mut rng = rand::rng();
        for _ in 0..1000 {
            let len = rng.random_range(1..=MAX_LETTERS);
            let s: String = (0..len)
                .map(|_| char::from(b'a' + rng.random_range(0..26u8)))
                .collect();
            assert_eq!(id(&s).letters(), s);
        }
    }

    #[test]
    fn test_invalid_packed_values() {
        for packed in [0, -1, 1, 4206, MIN_PACKED - 1, MAX_PACKED + 1] {
            assert_eq!(NoteId::from_packed(packed).try_letters(), None, "{packed}");
        }
        // `a`, an empty place, then `a`
        assert_eq!(
            NoteId::from_packed(MIN_PACKED + RADIX.pow(5)).try_letters(),
            None
        );
        assert_eq!(NoteId::from_packed(42).letters(), "#42");
    }

    #[test]
    fn test_parse() {
        assert_eq!(id("GIR"), id("gir"));
        assert_eq!(id("Gir"), id("gir"));
        assert_eq!(id(" gir "), id("gir"));
        let err = "42".parse::<NoteId>().unwrap_err().to_string();
        assert!(err.contains(ID_MAP_FILE_NAME), "{err}");
        for bad in ["", "abcdefghi", "g1r", "gi-r", "gïr"] {
            assert!(bad.parse::<NoteId>().is_err(), "{bad}");
        }
    }

    #[test]
    fn test_order_matches_strings() {
        let sorted = [
            "a", "aaa", "aay", "aaza", "aba", "giq", "giy", "giza", "gizb", "gizy", "gizzaa",
            "gizzaz", "gizzba", "gja", "szy", "szza", "taaa", "xzzy", "yaaaa", "zaaaaa",
            "zzzzzzzz",
        ];
        let ids: Vec<NoteId> = sorted.iter().map(|s| id(s)).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        let mut rng = rand::rng();
        let mut strings: Vec<String> = (0..2000)
            .map(|_| {
                let len = rng.random_range(1..=MAX_LETTERS);
                (0..len)
                    .map(|_| char::from(b'a' + rng.random_range(0..26u8)))
                    .collect()
            })
            .collect();
        strings.sort();
        strings.dedup();
        let ids: Vec<NoteId> = strings.iter().map(|s| id(s)).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn test_laptop_allocation() {
        assert_eq!(
            letters(&ids_after(LAPTOP, None, 3).unwrap()),
            ["aaa", "aab", "aac"]
        );
        assert_eq!(
            letters(&ids_after(LAPTOP, Some(id("giq")), 3).unwrap()),
            ["gir", "gis", "git"]
        );
        // After a server note, the laptop starts the next block.
        assert_eq!(
            letters(&ids_after(LAPTOP, Some(id("giza")), 1).unwrap()),
            ["gja"]
        );
        assert_eq!(
            letters(&ids_after(LAPTOP, Some(id("giy")), 1).unwrap()),
            ["gja"]
        );
        // Fits exactly in what is left of the block
        assert_eq!(
            letters(&ids_after(LAPTOP, Some(id("giq")), 8).unwrap())
                .last()
                .unwrap(),
            "giy"
        );
        // Does not fit, so it starts at the next block and fills as many as it needs.
        let import = letters(&ids_after(LAPTOP, Some(id("giq")), 60).unwrap());
        assert_eq!(import.first().unwrap(), "gja");
        assert_eq!(import[24], "gjy");
        assert_eq!(import[25], "gka");
        assert_eq!(import.last().unwrap(), "glj");
        assert!(import.iter().all(|s| s.len() == 3));
        // Tier boundaries
        assert_eq!(
            letters(&ids_after(LAPTOP, Some(id("szy")), 1).unwrap()),
            ["taaa"]
        );
        assert_eq!(
            letters(&ids_after(LAPTOP, Some(id("xzzy")), 1).unwrap()),
            ["yaaaa"]
        );
        assert_eq!(
            letters(&ids_after(LAPTOP, Some(id("yzzzy")), 1).unwrap()),
            ["zaaaaa"]
        );
        assert!(ids_after(LAPTOP, Some(id("zzzzzy")), 1).is_err());
        // Blocks may contain `z` after their first letter.
        assert_eq!(
            letters(&ids_after(LAPTOP, Some(id("ayy")), 1).unwrap()),
            ["aza"]
        );
        assert_eq!(
            letters(&ids_after(LAPTOP, Some(id("azy")), 1).unwrap()),
            ["baa"]
        );
        // Odd ids still give a later one.
        for odd in ["g", "gi", "giz", "gira", "gizzzz", "t", "ta"] {
            let next = ids_after(LAPTOP, Some(id(odd)), 1).unwrap()[0];
            assert!(next > id(odd), "{odd} -> {}", next.letters());
        }
    }

    #[test]
    fn test_server_allocation() {
        assert_eq!(
            letters(&ids_after(SERVER, Some(id("giq")), 2).unwrap()),
            ["giza", "gizb"]
        );
        assert_eq!(
            letters(&ids_after(SERVER, Some(id("gizy")), 1).unwrap()),
            ["gizzaa"]
        );
        assert_eq!(
            letters(&ids_after(SERVER, Some(id("gizzaz")), 1).unwrap()),
            ["gizzba"]
        );
        assert_eq!(
            letters(&ids_after(SERVER, Some(id("gizzyz")), 1).unwrap()),
            ["gizzzaaa"]
        );
        // Allocation errors instead of producing a 9th letter.
        assert!(ids_after(SERVER, Some(id("gizzzaaa")), 1).is_ok());
        assert!(ids_after(SERVER, Some(id("gizzzyzz")), 1).is_err());
        assert_eq!(
            letters(&ids_after(SERVER, Some(id("taazy")), 1).unwrap()),
            ["taazzaa"]
        );
        assert!(ids_after(SERVER, Some(id("taazzyz")), 1).is_err());
        for odd in ["g", "giz", "gizz", "gizza", "gizzzab"] {
            let next = ids_after(SERVER, Some(id(odd)), 1).unwrap()[0];
            assert!(next > id(odd), "{odd} -> {}", next.letters());
        }
    }

    #[test]
    fn test_other_replicas_cannot_allocate() {
        assert!(ids_after(Replica::new_unchecked(2), None, 1).is_err());
    }

    /// A replica's view of the notes: what it has created or seen, and its counter.
    #[derive(Default)]
    struct Side {
        seen: BTreeSet<NoteId>,
        counter: Option<NoteId>,
    }

    impl Side {
        fn create(&mut self, replica: Replica, n: usize) -> Vec<NoteId> {
            let newest = self.seen.last().copied().max(self.counter);
            let ids = ids_after(replica, newest, n).unwrap();
            self.counter = ids.last().copied();
            self.seen.extend(ids.iter().copied());
            ids
        }
    }

    fn is_prefix(a: NoteId, b: NoteId) -> bool {
        b.letters().starts_with(&a.letters())
    }

    /// Two replicas creating notes in random interleavings, with random syncs.
    #[test]
    fn test_two_replicas_random() {
        let mut rng = rand::rng();
        for _ in 0..200 {
            // Both start from the same notes, as after the first full upload.
            let mut laptop = Side::default();
            let seed = laptop.create(LAPTOP, rng.random_range(1..30));
            let mut server = Side {
                seen: laptop.seen.clone(),
                counter: None,
            };
            // Every note, the calls that created it, and the notes created before each sync
            let mut all: Vec<NoteId> = seed.clone();
            let mut calls: Vec<(usize, Vec<NoteId>)> = vec![(0, seed.clone())];
            let mut synced_before: Vec<NoteId> = seed;
            for _ in 0..rng.random_range(1..60) {
                match rng.random_range(0..10) {
                    0..=4 => {
                        let n = if rng.random_bool(0.2) {
                            rng.random_range(1..80)
                        } else {
                            rng.random_range(1..4)
                        };
                        let ids = laptop.create(LAPTOP, n);
                        assert!(ids.iter().all(|id| synced_before.iter().all(|s| s < id)));
                        all.extend(&ids);
                        calls.push((0, ids));
                    }
                    5..=7 => {
                        let ids = server.create(SERVER, rng.random_range(1..4));
                        assert!(ids.iter().all(|id| synced_before.iter().all(|s| s < id)));
                        all.extend(&ids);
                        calls.push((1, ids));
                    }
                    _ => {
                        let union: BTreeSet<NoteId> =
                            laptop.seen.union(&server.seen).copied().collect();
                        laptop.seen.clone_from(&union);
                        server.seen = union;
                        synced_before.clone_from(&all);
                    }
                }
            }
            let unique: BTreeSet<NoteId> = all.iter().copied().collect();
            assert_eq!(unique.len(), all.len(), "an id repeated");
            let sorted: Vec<NoteId> = unique.into_iter().collect();
            for (i, a) in sorted.iter().enumerate() {
                for b in &sorted[i + 1..] {
                    assert!(!is_prefix(*a, *b), "{a} is the start of {b}");
                }
            }
            // No other replica's note sorts inside a call.
            for (replica, ids) in &calls {
                let (lo, hi) = (ids[0], *ids.last().unwrap());
                for (other, other_ids) in &calls {
                    if other == replica {
                        continue;
                    }
                    assert!(
                        other_ids.iter().all(|id| *id < lo || *id > hi),
                        "{ids:?} has {other_ids:?} inside it"
                    );
                }
            }
        }
    }

    async fn insert_note(pool: &SqlitePool, id: NoteId) {
        sqlx::query("INSERT INTO note (id, data, custom_data, parser_id) VALUES (?, '', '{}', 1)")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }

    #[sqlx::test]
    async fn test_allocate_never_reuses_deleted_ids(pool: SqlitePool) {
        sqlx::query("INSERT INTO parser (name) VALUES ('markdown')")
            .execute(&pool)
            .await
            .unwrap();
        let first = allocate_note_ids(&pool, 3).await.unwrap();
        assert_eq!(letters(&first), ["aaa", "aab", "aac"]);
        for id in &first {
            insert_note(&pool, *id).await;
        }
        sqlx::query("DELETE FROM note")
            .execute(&pool)
            .await
            .unwrap();
        let second = allocate_note_ids(&pool, 1).await.unwrap();
        assert_eq!(letters(&second), ["aad"]);
        // Ids never inserted are not handed out again either.
        let third = allocate_note_ids(&pool, 1).await.unwrap();
        assert_eq!(letters(&third), ["aae"]);
        // A note from another replica moves the laptop to the next block.
        insert_note(&pool, id("aaza")).await;
        assert_eq!(
            letters(&allocate_note_ids(&pool, 1).await.unwrap()),
            ["aba"]
        );
    }

    #[sqlx::test]
    async fn test_concurrent_allocations_do_not_collide(pool: SqlitePool) {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let pool = pool.clone();
                tokio::spawn(async move { allocate_note_ids(&pool, 5).await.unwrap() })
            })
            .collect();
        let mut all = BTreeSet::new();
        for handle in handles {
            for id in handle.await.unwrap() {
                assert!(all.insert(id), "{id} handed out twice");
            }
        }
        assert_eq!(all.len(), 40);
    }

    #[sqlx::test]
    async fn test_next_ids_are_congruent_and_never_reused(pool: SqlitePool) {
        let mut conn = pool.acquire().await.unwrap();
        // Rows from before sync keep their ids, and new ones go above them.
        sqlx::query("INSERT INTO parser (id, name) VALUES (13, 'markdown')")
            .execute(&mut *conn)
            .await
            .unwrap();
        let laptop = next_ids_for(&mut conn, LAPTOP, IdTable::Parser, 3)
            .await
            .unwrap();
        assert_eq!(laptop, [16, 24, 32]);
        // Above the laptop's mark too, which a full sync brings to the server.
        let server = next_ids_for(&mut conn, SERVER, IdTable::Parser, 2)
            .await
            .unwrap();
        assert_eq!(server, [33, 41]);
        sqlx::query("DELETE FROM parser")
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            next_ids_for(&mut conn, LAPTOP, IdTable::Parser, 1)
                .await
                .unwrap(),
            [48]
        );
        // Without the other replica's mark, only the rows it has seen count.
        sqlx::query("DELETE FROM id_counter WHERE replica = 0")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO parser (id, name) VALUES (56, 'typst')")
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            next_ids_for(&mut conn, SERVER, IdTable::Parser, 1)
                .await
                .unwrap(),
            [57]
        );
        // Each table has its own counter.
        assert_eq!(
            next_ids_for(&mut conn, SERVER, IdTable::Tag, 1)
                .await
                .unwrap(),
            [1]
        );
        assert_eq!(
            next_ids_for(&mut conn, LAPTOP, IdTable::Card, 0)
                .await
                .unwrap(),
            Vec::<i64>::new()
        );
    }

    #[sqlx::test]
    async fn test_next_ids_from_interleaved_replicas_never_collide(pool: SqlitePool) {
        let mut rng = rand::rng();
        let mut seen = BTreeSet::new();
        let mut conn = pool.acquire().await.unwrap();
        for _ in 0..300 {
            let replica = Replica::new_unchecked(rng.random_range(0..3));
            let n = rng.random_range(1..4);
            let ids = next_ids_for(&mut conn, replica, IdTable::ReviewLog, n)
                .await
                .unwrap();
            assert_eq!(ids.len(), n);
            for id in ids {
                assert_eq!(id % ID_STEP, i64::from(replica.number()));
                assert!(seen.insert(id), "{id} handed out twice");
            }
            // Sometimes the rows are inserted (seen by every replica), sometimes deleted again.
            if rng.random_bool(0.3) {
                sqlx::query("DELETE FROM review_log")
                    .execute(&mut *conn)
                    .await
                    .unwrap();
            }
        }
    }

    #[sqlx::test]
    async fn test_concurrent_next_ids_do_not_collide(pool: SqlitePool) {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let pool = pool.clone();
                tokio::spawn(async move { next_ids_in(&pool, IdTable::Card, 5).await.unwrap() })
            })
            .collect();
        let mut all = BTreeSet::new();
        for handle in handles {
            for id in handle.await.unwrap() {
                assert!(all.insert(id), "{id} handed out twice");
            }
        }
        assert_eq!(all.len(), 40);
    }

    #[sqlx::test]
    async fn test_ensure_note_ids_migrated(pool: SqlitePool) {
        sqlx::query("INSERT INTO parser (name) VALUES ('markdown')")
            .execute(&pool)
            .await
            .unwrap();
        insert_note(&pool, NoteId::from_packed(7)).await;
        assert!(ensure_note_ids_migrated(&pool).await.is_err());
        sqlx::query("DELETE FROM note")
            .execute(&pool)
            .await
            .unwrap();
        // An empty database has nothing to migrate.
        ensure_note_ids_migrated(&pool).await.unwrap();
        insert_note(&pool, NoteId::from_packed(7)).await;
        // The backstop: the flag is set but a numeric id is left.
        assert!(ensure_note_ids_migrated(&pool).await.is_err());
    }
}
