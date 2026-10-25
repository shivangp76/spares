//! Note ids: short lowercase letter strings (`gir`, `girh`) that are unique across replicas without
//! coordination, sort in creation order, and keep notes created together next to each other.
//!
//! # Requirements
//!
//! 1. **Unique without coordination**, so an offline laptop or phone can still create notes.
//! 2. **Short.** Most notes are created on the laptop, so its ids are 3 letters for the next 8,000
//!    or so (5–8 years at 1,000–1,500 a year), then 4 for decades. Notes from the web or a phone
//!    are one letter longer than the laptop's, for their first 5 or 6 after each laptop note. No
//!    id is longer than 8 letters.
//! 3. **Letters only.** macOS Finder sorts runs of digits as numbers, so mixed letters and digits
//!    would sort wrongly, and the phone needs no number keyboard.
//! 4. **Lowercase only**, which is safe on case-insensitive file systems. Input is accepted in any
//!    case, because phone keyboards capitalize the first letter.
//! 5. **Notes created together sort next to each other**, even when another replica creates notes
//!    at the same time, so related notes can be browsed with the arrow keys.
//! 6. **Ids follow creation order across devices.** A note created after a sync sorts after
//!    everything either side created before that sync. Notes created on both sides while out of
//!    sync are ordered by position rather than time. The one exception: notes that two replicas
//!    other than the laptop create after the same laptop note are in replica order.
//! 7. **Never reused**, even after a delete or an undo.
//! 8. **Derived from the data, not the clock**, so clock changes and full syncs can't break it.
//!
//! # Format
//!
//! ```text
//! laptop:  aaa, aab, …, aaz, aba, …, szz, taaa, …
//! others:  <the laptop note it follows><a letter of its lane>   e.g. after "giq": "giqa", "giqh"
//! ```
//!
//! - **The laptop's ids are one sequence**, in which every string of the right length is used in
//!   turn. **The first letter decides the length** ([`laptop_len`]), so ids grow instead of running
//!   out:
//!
//!   | First letter | Laptop id | Laptop ids | Others' ids |
//!   |---|---|---|---|
//!   | `a`–`s` | 3 letters | 12,844 | 4 |
//!   | `t`–`x` | 4 | 87,880 | 5 |
//!   | `y` | 5 | 456,976 | 6 |
//!   | `z` | 6 | 11.9 million | 7 |
//!
//!   A laptop id is never the start of another laptop id, since all those starting with one letter
//!   have one length.
//! - **Every other replica follows the laptop's newest note** that it has seen, its *anchor*, with
//!   letters of its own, its *lane* ([`lane`]):
//!
//!   | Replica | Lane | 1 letter after the anchor | Then |
//!   |---|---|---|---|
//!   | `1`, the server | `a`–`g` | `a`–`f` | `g` + a tail |
//!   | `2`, a device | `h`–`n` | `h`–`m` | `n` + a tail |
//!   | `3`, a device | `o`–`t` | `o`–`s` | `t` + a tail |
//!   | `4`, a device | `u`–`z` | `u`–`y` | `z` + a tail |
//!
//!   The lane's last letter is followed by a tail ([`next_tail`]): `a`–`y` (25 ids one letter
//!   longer), then `zaa`–`zyz` (650 more, three letters longer). So the server's notes after `giq`
//!   are `giqa`, …, `giqf`, `giqga`, …, `giqgy`, `giqgzaa`, ….
//! - **So the only id that starts another is the laptop note an id follows**, and a laptop note
//!   sorts before the notes that follow it. Byte order and Finder's order
//!   (`localizedStandardCompare`) agree, also on file names such as `giq.typ`, `giq-1-front.pdf`,
//!   `giqa.typ`, `giqgzaa.typ`, `giqh.typ`, `gir.typ`.
//! - Creation time stays in `created_at`.
//!
//! # Allocation
//!
//! Each replica keeps a cursor, the last note id it handed out (see [the high-water
//! mark](#the-high-water-mark)). **M** is the larger of the cursor and the largest note id in the
//! table: the newest note this replica has created or seen. Its anchor is the laptop note M is or
//! follows ([`anchor_of`]).
//!
//! - **The laptop takes the next laptop ids after M's anchor:** after `giq` or `giqh`, `gir`,
//!   `gis`, …, `giz`, `gja`, …. A call takes as many as it needs, in order, whatever it crosses.
//! - **Another replica takes the next ids of its lane after M's anchor**: after its own newest note
//!   there if it has one (its cursor, or a note of its lane in the table), else from the start of
//!   its lane. After `giq`, the server takes `giqa`, `giqb`, …, and replica 2 `giqh`, `giqi`, …,
//!   until they see a newer laptop note.
//!
//! Why this meets the requirements:
//! - **Unique:** the laptop writes laptop ids. Anything else is longer than a laptop id starting
//!   with its letter, and its letter after the anchor says whose it is. No replica's cursor goes
//!   back.
//! - **Created together, sorted together:** a laptop call takes consecutive laptop ids. Another
//!   replica follows only laptop notes it has seen, and it sees a call whole, so its notes sort
//!   before the call (following an earlier note) or after it (following its last). Another
//!   replica's call is one run of its lane after one anchor, and lanes don't overlap.
//! - **Creation order:** the laptop allocates above M. Another replica allocates above M's anchor,
//!   above everything its own lane had there, and above every lane before its own. Notes of a lane
//!   after its own, after the same anchor, are the exception in requirement 6.
//!
//! **Cost.** None for the laptop: it never skips an id. Another replica's notes are a letter longer
//! than the laptop's for its first 5 or 6 after each laptop note it sees, 2 letters longer for the
//! next 25 and 4 for the next 650. Allocation returns an error rather than produce a 9th letter,
//! which a replica reaches after about 680 notes on one laptop note while the laptop's ids are 3
//! or 4 letters, and after about 30 once they are 5 or 6 (some 100,000 laptop notes from now).
//!
//! [`allocate_note_ids`] gets all of a call's notes at once, so an import gets one run of ids. Undo
//! and redo replay the ids stored in the event and don't allocate.
//!
//! **Three devices at most** (replicas `2`–`4`, see [`crate::sync::devices`]): each lane takes
//! letters from the others, so more devices would mean fewer 4-letter ids for each. Replicas
//! `5`–`7` create no notes. A second laptop would be enrolled as a device, and its notes would be
//! a letter longer than the first's.
//!
//! # The high-water mark
//!
//! "Largest id in the table + 1" hands out a deleted note's id again once the newest notes are
//! deleted. Two unrelated notes would then share one file path in the notes repository's history,
//! "create, undo, create, redo" would fail on the primary key, and sync would carry a grave and a
//! new note with the same id.
//!
//! So `id_counter(replica, tbl, last)` stores, per replica and table, the largest id that replica
//! ever handed out. Deletes never lower it. It's keyed by replica because full syncs copy the
//! whole database, including the other side's rows. The next id starts from
//! `max(last, largest in the table)`, which counts ids that arrived from the other replica.
//!
//! - **Locking.** The allocation's first statement is a no-op write on its `id_counter` row, so the
//!   transaction takes SQLite's write lock before it reads the table's maximum. With sqlx's default
//!   deferred transaction, reading first would let two concurrent creates read the same maximum.
//! - **A full sync never lowers the receiving replica's own counter.** `id_counter` rows are merged
//!   by taking the larger value. Otherwise a full download could make the laptop hand out again an
//!   id it allocated and deleted before the sync.
//!
//! # Storage
//!
//! Ids stay `INTEGER PRIMARY KEY`. The letters are stored as a base-27 number left-aligned to
//! [`MAX_LETTERS`] places: a letter is its position + 1 (`a` = 1 … `z` = 26), and an empty place
//! is 0. So `gir` is 7·27⁷ + 9·27⁶ + 18·27⁵.
//!
//! - Comparing the integers compares the strings, whatever their lengths, and the database schema
//!   and JSON wire format keep integers.
//! - The packing doesn't depend on the allocation rules, so they can change without touching
//!   stored ids, and decoding (frontend, nvim) needs no knowledge of them.
//! - The largest value, `zzzzzzzz` ([`MAX_PACKED`]), is 27⁸ − 1 (39 bits), exact as a JS or Lua
//!   number. The frontend must decode with division and `Math.floor`, since JS `>>` works on 32
//!   bits.
//! - The smallest id, `a` ([`MIN_PACKED`]), is 27⁷, far above the old numeric ids, so a note id
//!   below 27⁷ can only be unmigrated. No valid id packs to `0`, so there are no sentinel ids.
//! - [`NoteId`] is a newtype; other ids stay `i64`. Any letter string parses, whether or not the
//!   allocator could produce it. Digits are rejected, with a hint to look the old id up in
//!   [`ID_MAP_FILE_NAME`].
//!
//! # Other tables
//!
//! Cards, review logs, tags and parsers have no human-facing requirement, but still need ids
//! unique across replicas. New rows take the smallest id above `max(high-water marks, MAX(id))`
//! that is congruent to `SPARES_REPLICA` mod [`ID_STEP`] ([`next_ids`]).
//!
//! - The modulus is fixed at 8, not the current replica count, so adding a replica never changes
//!   the rule for existing ones. Wasting 7/8 of the numbers costs nothing, since these ids are
//!   never shown.
//! - Existing rows kept their ids when note ids were migrated, so review logs, the phone's offline
//!   queue and undo payloads still refer to the right cards.
//! - `note_tag`, `card_tag`, `note_link` and `note_keyword` keep SQLite's own row ids. They are
//!   identified by their parent row and contents.
//!
//! # Alternatives considered
//!
//! - **Batches of 26** (the scheme before letter blocks): every existing note and every new single
//!   note was 4 letters, imports over 26 notes (half of the notes created since 2025) were 5,
//!   every call took a whole batch even for one note, and parity reserved half the batches for a
//!   server that rarely creates notes.
//! - **Blocks with a `z` tail** (the scheme this replaced): laptop ids were a 2-letter block and a
//!   slot `a`–`y`, and other replicas' ids `<block>z<tail>`, after every slot of the block. Each
//!   web note cost the laptop the rest of its block (about 12 ids), a call that didn't fit in a
//!   block skipped to the next (about 15% of ids), and every other replica shared the 25 one-letter
//!   tails of a block: split by last letter, they interleaved; split into ranges, two replicas
//!   taking turns ran out within a few turns.
//! - **Keeping strict creation order between other replicas** after one anchor: a replica that
//!   sees a newer note of a later lane would have to take a longer id after it, and two replicas
//!   taking turns would soon reach the 8-letter limit. Their notes after one laptop note are few,
//!   so replica order is kept instead.
//! - **No vowels (base 21)**, so ids never spell words, including rude ones. It makes ids longer.
//!   If some ids turn out annoying, the allocator can skip a denylist of ids at any time; a
//!   skipped id costs capacity, never correctness.

pub mod migrate;

use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;
use sqlx::AssertSqlSafe;
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
/// Letters the first letter of a [tail](next_tail) can take: `a`–`y`. `z` starts the next level.
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

/// Length of a laptop id whose first letter is `first`: 3 for `a`–`s`, 4 for `t`–`x`, 5 for `y`
/// and 6 for `z`. Laptop ids grow instead of running out.
fn laptop_len(first: u8) -> usize {
    match first {
        0..=18 => 3,
        19..=23 => 4,
        24 => 5,
        _ => 6,
    }
}

/// The laptop id after `id`, moving to the next tier's length when the first letter changes tier.
fn next_laptop_id(id: &[u8]) -> Result<Vec<u8>, Error> {
    let mut next = id.to_vec();
    for i in (0..next.len()).rev() {
        if next[i] < Z {
            next[i] += 1;
            if i == 0 {
                next.resize(laptop_len(next[0]), 0);
            }
            return Ok(next);
        }
        next[i] = 0;
    }
    Err(note_error(
        "Ran out of note ids: every laptop id has been used".to_string(),
    ))
}

/// The laptop id `id` starts with: `id` itself for a laptop id, the note it follows for another
/// replica's. A shorter id is padded with `a`, which gives the smallest laptop id after it.
fn anchor_of(id: &[u8]) -> Vec<u8> {
    let mut anchor = id[..id.len().min(laptop_len(id[0]))].to_vec();
    anchor.resize(laptop_len(id[0]), 0);
    anchor
}

/// `n` laptop ids after `newest`, the newest note this replica has created or seen.
fn laptop_ids_after(newest: Option<&[u8]>, n: usize) -> Result<Vec<Vec<u8>>, Error> {
    let mut next = match newest {
        None => vec![0; laptop_len(0)],
        Some(id) => {
            let anchor = anchor_of(id);
            if anchor.as_slice() > id {
                anchor
            } else {
                next_laptop_id(&anchor)?
            }
        }
    };
    let mut ids = Vec::with_capacity(n);
    for _ in 0..n {
        ids.push(next.clone());
        next = next_laptop_id(&next)?;
    }
    Ok(ids)
}

/// The letters a replica other than the laptop appends to the laptop note it follows, or `None`
/// for one that creates no notes: the laptop's ids are its own, and replicas `5`–`7` have no
/// lane. The last letter is the lane's "more" letter: it is followed by a [tail](next_tail) rather
/// than being an id itself.
fn lane(replica: u8) -> Option<std::ops::RangeInclusive<u8>> {
    match replica {
        1 => Some(0..=6),   // a–g: the server
        2 => Some(7..=13),  // h–n
        3 => Some(14..=19), // o–t
        4 => Some(20..=25), // u–z
        _ => None,
    }
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

/// The suffix of `lane` after `suffix`, or its first one. The lane's letters but the last come
/// first, then the last followed by the [tails](next_tail): `a`, …, `f`, `ga`, …, `gy`, `gzaa`, ….
fn next_suffix(lane: &std::ops::RangeInclusive<u8>, suffix: Option<&[u8]>) -> Vec<u8> {
    let (start, more) = (*lane.start(), *lane.end());
    match suffix {
        None | Some([]) => vec![start],
        Some([letter]) if *letter + 1 < more => vec![letter + 1],
        Some([_]) => vec![more, 0],
        Some([_, tail @ ..]) => {
            let mut next = vec![more];
            next.extend(next_tail(tail));
            next
        }
    }
}

/// `n` ids of `replica` (with lane `lane`) after the laptop note `newest` starts with, after
/// `own`, its newest note there, if it has one.
fn lane_ids_after(
    lane: &std::ops::RangeInclusive<u8>,
    newest: Option<&[u8]>,
    own: Option<&[u8]>,
    n: usize,
) -> Vec<Vec<u8>> {
    let anchor = newest.map_or_else(|| vec![0; laptop_len(0)], anchor_of);
    let mut suffix = own
        .filter(|own| own.len() > anchor.len() && own.starts_with(&anchor))
        .filter(|own| lane.contains(&own[anchor.len()]))
        .map(|own| own[anchor.len()..].to_vec());
    let mut ids = Vec::with_capacity(n);
    for _ in 0..n {
        let next = next_suffix(lane, suffix.as_deref());
        let mut id = anchor.clone();
        id.extend_from_slice(&next);
        ids.push(id);
        suffix = Some(next);
    }
    ids
}

/// The smallest and largest ids of `replica`'s lane after the laptop note `newest` starts with, as
/// packed values, to find its newest note there.
fn lane_bounds(replica: Replica, newest: Option<NoteId>) -> Option<(i64, i64)> {
    let lane = lane(replica.number())?;
    let anchor = newest
        .and_then(NoteId::letter_indices)
        .map_or_else(|| vec![0; laptop_len(0)], |id| anchor_of(&id));
    let mut lo = anchor.clone();
    lo.push(*lane.start());
    let mut hi = anchor;
    hi.push(*lane.end());
    hi.resize(MAX_LETTERS.max(hi.len()), Z);
    Some((pack(&lo)?, pack(&hi)?))
}

/// `n` ids `replica` would hand out after `newest`, the newest note it has created or seen. `own`
/// is its newest note after the laptop note `newest` starts with, for a replica other than the
/// laptop (see [`lane_bounds`]). Pure, so it can be tested without a database.
fn ids_after(
    replica: Replica,
    newest: Option<NoteId>,
    own: Option<NoteId>,
    n: usize,
) -> Result<Vec<NoteId>, Error> {
    let letters = |id: NoteId| {
        id.letter_indices().ok_or_else(|| {
            note_error(format!(
                "Cannot allocate note ids after {}, which is not a letter id. Run `spares \
                 migrate ids` first.",
                id.packed()
            ))
        })
    };
    let newest = newest.map(letters).transpose()?;
    let own = own.map(letters).transpose()?;
    let ids = match (replica.number(), lane(replica.number())) {
        (0, _) => laptop_ids_after(newest.as_deref(), n)?,
        (_, Some(lane)) => {
            let ids = lane_ids_after(&lane, newest.as_deref(), own.as_deref(), n);
            if ids.last().is_some_and(|id| id.len() > MAX_LETTERS) {
                let anchor = newest
                    .as_deref()
                    .map_or_else(|| vec![0; laptop_len(0)], anchor_of);
                return Err(note_error(format!(
                    "Ran out of note ids after the laptop's note `{}`: this replica has created as \
                     many notes after it as {MAX_LETTERS} letters allow. Sync a newer note from \
                     the laptop, or create fewer at once.",
                    NoteId::from_letter_indices(&anchor)?.letters()
                )));
            }
            ids
        }
        (number, None) => {
            return Err(note_error(format!(
                "Replica {number} cannot create notes: only the laptop (0), the server (1) and \
                 devices 2 to 4 have note id rules. See \"Allocation\" in the `spares_core::ids` \
                 docs."
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
    let counter: Option<i64> = sqlx::query_scalar(
        "SELECT last FROM id_counter WHERE replica = ? AND tbl = 'note' AND last > 0",
    )
    .bind(replica_number)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let table_max: Option<i64> = sqlx::query_scalar("SELECT MAX(id) FROM note")
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    let newest = counter.max(table_max).map(NoteId::from_packed);
    // A replica other than the laptop continues after its own newest note on that laptop note: its
    // counter, or a note of its lane in the table (one it made before a full download, say).
    let own = match lane_bounds(replica, newest) {
        None => None,
        Some((lo, hi)) => {
            let in_table: Option<i64> =
                sqlx::query_scalar("SELECT MAX(id) FROM note WHERE id BETWEEN ? AND ?")
                    .bind(lo)
                    .bind(hi)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })?;
            in_table
                .max(counter.filter(|c| (lo..=hi).contains(c)))
                .map(NoteId::from_packed)
        }
    };
    let ids = ids_after(replica, newest, own, n)?;
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
    SavedSearch,
}

impl IdTable {
    pub const ALL: [Self; 5] = [
        Self::Card,
        Self::ReviewLog,
        Self::Tag,
        Self::Parser,
        Self::SavedSearch,
    ];

    /// The table's name, which is also its `id_counter.tbl` key.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Card => "card",
            Self::ReviewLog => "review_log",
            Self::Tag => "tag",
            Self::Parser => "parser",
            Self::SavedSearch => "saved_search",
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
    let last: i64 = sqlx::query_scalar(AssertSqlSafe(query))
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
    /// Browsers that keep a copy of the collection
    const DEVICE: Replica = Replica::new_unchecked(2);
    const DEVICE_4: Replica = Replica::new_unchecked(4);

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
        let after =
            |newest: Option<&str>, n| letters(&ids_after(LAPTOP, newest.map(id), None, n).unwrap());
        assert_eq!(after(None, 3), ["aaa", "aab", "aac"]);
        assert_eq!(after(Some("giq"), 3), ["gir", "gis", "git"]);
        // After another replica's notes on `giq`, the next laptop id: nothing is skipped.
        assert_eq!(after(Some("giqa"), 1), ["gir"]);
        assert_eq!(after(Some("giqhzaa"), 1), ["gir"]);
        // Every letter, then the next run of 26; an import just continues.
        assert_eq!(after(Some("giy"), 3), ["giz", "gja", "gjb"]);
        let import = after(Some("giq"), 60);
        assert_eq!((import[0].as_str(), import[59].as_str()), ("gir", "gky"));
        assert!(import.iter().all(|s| s.len() == 3));
        // Tier boundaries
        assert_eq!(after(Some("szz"), 1), ["taaa"]);
        assert_eq!(after(Some("szzh"), 1), ["taaa"]);
        assert_eq!(after(Some("xzzz"), 1), ["yaaaa"]);
        assert_eq!(after(Some("yzzzz"), 1), ["zaaaaa"]);
        assert!(ids_after(LAPTOP, Some(id("zzzzzz")), None, 1).is_err());
        // Odd ids still give a later one.
        for odd in ["g", "gi", "t", "ta", "taa", "taaaa"] {
            let next = ids_after(LAPTOP, Some(id(odd)), None, 1).unwrap()[0];
            assert!(next > id(odd), "{odd} -> {}", next.letters());
        }
    }

    #[test]
    fn test_lane_allocation() {
        let after = |replica, newest: Option<&str>, own: Option<&str>, n| {
            letters(&ids_after(replica, newest.map(id), own.map(id), n).unwrap())
        };
        // After the laptop's `giq`, each replica's lane, then its "more" letter and the tails.
        assert_eq!(
            after(SERVER, Some("giq"), None, 8),
            [
                "giqa", "giqb", "giqc", "giqd", "giqe", "giqf", "giqga", "giqgb"
            ]
        );
        assert_eq!(after(DEVICE, Some("giq"), None, 2), ["giqh", "giqi"]);
        assert_eq!(
            after(DEVICE_4, Some("giq"), None, 6),
            ["giqu", "giqv", "giqw", "giqx", "giqy", "giqza"]
        );
        // Continuing after its own newest note on that laptop note, whoever's note is newest
        assert_eq!(after(DEVICE, Some("giqb"), Some("giqi"), 1), ["giqj"]);
        assert_eq!(after(DEVICE, Some("giqu"), Some("giqm"), 1), ["giqna"]);
        assert_eq!(after(SERVER, Some("giqh"), None, 1), ["giqa"]);
        assert_eq!(after(SERVER, Some("giqgy"), Some("giqgy"), 1), ["giqgzaa"]);
        // A newer laptop note starts again at the lane's first letter, whatever came before.
        assert_eq!(after(DEVICE, Some("gis"), Some("giqk"), 1), ["gish"]);
        assert_eq!(after(SERVER, None, None, 1), ["aaaa"]);
        // A longer laptop id gets a longer tail.
        assert_eq!(after(DEVICE, Some("taaa"), None, 1), ["taaah"]);
        // Too long: an error, not a 9th letter
        let err = ids_after(SERVER, Some(id("giq")), Some(id("giqgzyzz")), 1).unwrap_err();
        assert!(
            err.to_string().contains("after the laptop's note `giq`"),
            "{err}"
        );
        // The other replicas have no lane.
        assert!(ids_after(Replica::new_unchecked(5), Some(id("giq")), None, 1).is_err());
        // Odd ids still give a later one.
        for odd in ["g", "gi", "giqg", "giqgz"] {
            let next = ids_after(SERVER, Some(id(odd)), Some(id(odd)), 1).unwrap()[0];
            assert!(next > id(odd), "{odd} -> {}", next.letters());
        }
    }

    /// Lanes don't overlap, and every letter is in one, so no id of one replica is another's.
    #[test]
    fn test_lanes_partition_the_letters() {
        let mut owners = vec![0; 26];
        for replica in 0..=Replica::MAX {
            for letter in lane(replica).into_iter().flatten() {
                owners[usize::from(letter)] += 1;
            }
        }
        assert_eq!(owners, vec![1; 26]);
        assert!(lane(0).is_none());
    }

    #[test]
    fn test_lane_bounds() {
        let (lo, hi) = lane_bounds(DEVICE, Some(id("giqa"))).unwrap();
        assert_eq!((lo, hi), (id("giqh").packed(), id("giqnzzzz").packed()));
        assert!(lane_bounds(LAPTOP, Some(id("giq"))).is_none());
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
            let own = lane_bounds(replica, newest).and_then(|(lo, hi)| {
                let in_lane = |id: &NoteId| (lo..=hi).contains(&id.packed());
                self.seen
                    .iter()
                    .copied()
                    .filter(in_lane)
                    .max()
                    .max(self.counter.filter(in_lane))
            });
            let ids = ids_after(replica, newest, own, n).unwrap();
            self.counter = ids.last().copied();
            self.seen.extend(ids.iter().copied());
            ids
        }
    }

    fn is_prefix(a: NoteId, b: NoteId) -> bool {
        b.letters().starts_with(&a.letters())
    }

    /// The laptop note `id` starts with, and the replica whose id it is.
    fn place(id: NoteId) -> (Vec<u8>, u8) {
        let letters = id.letter_indices().unwrap();
        let anchor = anchor_of(&letters);
        if letters.len() == anchor.len() {
            return (anchor, 0);
        }
        let letter = letters[anchor.len()];
        let owner = (1..=Replica::MAX).find(|r| lane(*r).is_some_and(|l| l.contains(&letter)));
        (anchor, owner.unwrap())
    }

    /// The laptop, the server and two devices creating notes in random interleavings. Each client
    /// syncs with the server at random.
    #[test]
    fn test_replicas_random() {
        let mut rng = rand::rng();
        let replicas = [LAPTOP, SERVER, DEVICE, DEVICE_4];
        for _ in 0..300 {
            // Everyone starts from the same notes, as after full downloads.
            let mut sides: Vec<Side> = (0..replicas.len()).map(|_| Side::default()).collect();
            let seed = sides[0].create(LAPTOP, rng.random_range(1..30));
            for side in &mut sides[1..] {
                side.seen = seed.iter().copied().collect();
            }
            // Every note, and the calls that created it
            let mut all: Vec<NoteId> = seed.clone();
            let mut calls: Vec<(usize, Vec<NoteId>)> = vec![(0, seed)];
            for _ in 0..rng.random_range(1..80) {
                let who = rng.random_range(0..replicas.len());
                if rng.random_bool(0.25) {
                    if who != 1 {
                        let union: BTreeSet<NoteId> =
                            sides[who].seen.union(&sides[1].seen).copied().collect();
                        sides[who].seen.clone_from(&union);
                        sides[1].seen = union;
                    }
                    continue;
                }
                let n = if who == 0 && rng.random_bool(0.2) {
                    rng.random_range(1..80)
                } else {
                    rng.random_range(1..8)
                };
                let before = sides[who].seen.clone();
                let ids = sides[who].create(replicas[who], n);
                // Creation order: after everything this replica has seen, except other replicas'
                // notes on the same laptop note in a lane after its own
                let (anchor, me) = place(ids[0]);
                for s in &before {
                    let (s_anchor, owner) = place(*s);
                    let higher_lane = me != 0 && owner > me && s_anchor == anchor;
                    assert!(*s < ids[0] || higher_lane, "{s} is not before {}", ids[0]);
                }
                all.extend(&ids);
                calls.push((who, ids));
            }
            let unique: BTreeSet<NoteId> = all.iter().copied().collect();
            assert_eq!(unique.len(), all.len(), "an id repeated");
            // An id starts another only as the laptop note the other follows.
            let sorted: Vec<NoteId> = unique.into_iter().collect();
            for (i, a) in sorted.iter().enumerate() {
                for b in &sorted[i + 1..] {
                    if is_prefix(*a, *b) {
                        assert_eq!(place(*a).1, 0, "{a} is the start of {b}");
                        assert_eq!(place(*b).0, a.letter_indices().unwrap());
                    }
                }
            }
            // No other replica's note sorts inside any call.
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
