//! The client's half of a normal sync: merging the server's changes into the local database.
//!
//! [`merge_pulled`] runs inside the transaction that also pushes and stamps (see
//! [`super::run`]), with the triggers off. It writes every row it takes from the server with the
//! pull's usn, and every row the server has to receive (local changes, merge results) with
//! `usn = -1`, so the push is simply everything dirty afterwards.
//!
//! The rules (section 6 of `docs/sync-plan.md`):
//! - A row changed on one side only takes that side's version, whatever its timestamps.
//! - Changed on both sides: a three-way merge against the last synced version ([`super::merge`]).
//!   Note text that both sides changed in the same lines keeps the local text, stores the other
//!   in `sync_conflict` and tags the note `sync-conflict`. Nothing is discarded.
//! - A note edited on one side and deleted on the other is kept, and tagged `sync-conflict`. A
//!   review does not count as an edit, and a card's existence follows its note's text, so card
//!   deletes win.
//! - The server never re-parses notes for sync, so the client decides which cards a note whose
//!   text both sides changed ends up with ([`reconcile_cards`]).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use chrono::DateTime;
use chrono::Utc;
use serde_json::Value;
use sqlx::SqliteConnection;

use super::SyncKind;
use super::apply::Applied;
use super::apply::delete_row;
use super::apply::insert_grave;
use super::apply::mark_dirty;
use super::apply::release_live_keys;
use super::apply::remove_grave;
use super::apply::row_exists;
use super::apply::upsert_card;
use super::apply::upsert_note;
use super::apply::upsert_parser;
use super::apply::upsert_review_log;
use super::apply::upsert_tag;
use super::merge::Side;
use super::merge::TextMerge;
use super::merge::f64_eq;
use super::merge::merge_json;
use super::merge::merge_sets;
use super::merge::merge_text;
use super::merge::merge3_or;
use super::orders;
use super::records::CardRecord;
use super::records::Changes;
use super::records::Grave;
use super::records::NoteRecord;
use super::records::ParserRecord;
use super::records::TagRecord;
use super::records::has_grave;
use super::records::read_base;
use super::records::read_card;
use super::records::read_note;
use super::records::read_parser;
use super::records::read_tag;
use super::records::read_usn;
use crate::Error;
use crate::api::fetch_review_logs_for_replay;
use crate::api::note::create_cards_on;
use crate::api::tag::DEFAULT_TAG_AUTO_DELETE;
use crate::config::read_external_config;
use crate::ids::IdTable;
use crate::ids::next_id;
use crate::model::Card;
use crate::model::CardId;
use crate::model::NoteId;
use crate::model::SpecialState;
use crate::model::TagId;
use crate::parsers::add_order_to_note_data;
use crate::parsers::find_parser;
use crate::parsers::get_all_parsers;
use crate::schedulers::get_default_scheduler_name;
use crate::schedulers::get_scheduler_from_string;

async fn parser_of(
    conn: &mut SqliteConnection,
    parser_id: i64,
) -> Result<Box<dyn crate::parsers::Parseable>, Error> {
    let name: String = sqlx::query_scalar("SELECT name FROM parser WHERE id = ?")
        .bind(parser_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    find_parser(&name, &get_all_parsers())
}

/// The tag put on notes that need a look after a sync: a text conflict, or an edit on one side
/// of a note the other side deleted.
pub const SYNC_CONFLICT_TAG: &str = "sync-conflict";

fn sqlx_error(e: sqlx::Error) -> Error {
    Error::Sqlx { source: e }
}

/// What merging the server's changes did locally.
#[derive(Debug, Default)]
pub struct MergeOutcome {
    pub applied: Applied,
    /// Notes now tagged `sync-conflict`.
    pub conflicts: BTreeSet<NoteId>,
}

struct Merger<'a> {
    conn: &'a mut SqliteConnection,
    pulled: &'a Changes,
    usn: i64,
    now: DateTime<Utc>,
    outcome: MergeOutcome,
    /// Notes deleted here and edited there, which come back.
    resurrected: BTreeSet<NoteId>,
    /// Notes whose text both sides changed, with the text they end up with.
    reconcile: BTreeMap<NoteId, String>,
    /// Cards reviewed on both sides, whose memory state is replayed from the merged reviews.
    replay: BTreeSet<CardId>,
}

/// Merges `pulled`, the server's changes since the last sync, into the local database, which
/// `conn` has in a transaction with the triggers off. `usn` is the pull's.
pub async fn merge_pulled(
    conn: &mut SqliteConnection,
    pulled: &Changes,
    usn: i64,
    now: DateTime<Utc>,
) -> Result<MergeOutcome, Error> {
    let mut merger = Merger {
        conn,
        pulled,
        usn,
        now,
        outcome: MergeOutcome::default(),
        resurrected: BTreeSet::new(),
        reconcile: BTreeMap::new(),
        replay: BTreeSet::new(),
    };
    for record in &pulled.parsers {
        merger.merge_parser(record).await?;
    }
    for record in &pulled.tags {
        merger.merge_tag(record).await?;
    }
    for grave in pulled.graves.iter().filter(|g| g.kind == SyncKind::Note) {
        merger
            .apply_note_grave(NoteId::from_packed(grave.oid))
            .await?;
    }
    let note_ids: Vec<NoteId> = pulled.notes.iter().map(|n| n.id).collect();
    release_live_keys(merger.conn, &note_ids).await?;
    for record in &pulled.notes {
        merger.merge_note(record).await?;
    }
    for record in &pulled.cards {
        merger.merge_card(record).await?;
    }
    for record in &pulled.review_logs {
        upsert_review_log(merger.conn, record, usn).await?;
    }
    merger.replay_cards().await?;
    for grave in pulled.graves.iter().filter(|g| g.kind == SyncKind::Card) {
        merger.apply_card_grave(grave.oid).await?;
    }
    let reconcile = std::mem::take(&mut merger.reconcile);
    for (note_id, text) in reconcile {
        reconcile_cards(merger.conn, note_id, &text, now).await?;
        merger.outcome.applied.notes_changed.insert(note_id);
    }
    for grave in pulled.graves.iter().filter(|g| g.kind == SyncKind::Tag) {
        merger.apply_tag_grave(grave.oid).await?;
    }
    for grave in pulled.graves.iter().filter(|g| g.kind == SyncKind::Parser) {
        merger.apply_parser_grave(grave.oid).await?;
    }
    let mut outcome = merger.outcome;
    let deleted = outcome.applied.notes_deleted.clone();
    outcome
        .applied
        .notes_changed
        .retain(|id| !deleted.contains(id));
    Ok(outcome)
}

impl Merger<'_> {
    async fn is_dirty(&mut self, kind: SyncKind, id: i64) -> Result<bool, Error> {
        Ok(read_usn(self.conn, kind, id).await? == Some(-1))
    }

    /// The usn a merged row is written with: the pull's if it is what the server has, otherwise
    /// dirty, so it is pushed.
    fn usn_for<T: PartialEq>(&self, merged: &T, remote: &T) -> i64 {
        if merged == remote { self.usn } else { -1 }
    }

    async fn merge_parser(&mut self, remote: &ParserRecord) -> Result<(), Error> {
        let merged = match read_parser(self.conn, remote.id).await? {
            Some(local) if self.is_dirty(SyncKind::Parser, remote.id).await? => {
                let base = read_base(self.conn, SyncKind::Parser, remote.id)
                    .await?
                    .unwrap_or_else(|| remote.clone());
                ParserRecord {
                    id: remote.id,
                    name: merge3_or(&base.name, &local.name, &remote.name, Side::Local),
                }
            }
            _ => remote.clone(),
        };
        remove_grave(
            self.conn,
            Grave {
                kind: SyncKind::Parser,
                oid: remote.id,
            },
        )
        .await?;
        let usn = self.usn_for(&merged, remote);
        upsert_parser(self.conn, &merged, usn).await
    }

    async fn merge_tag(&mut self, remote: &TagRecord) -> Result<(), Error> {
        let merged = match read_tag(self.conn, remote.id).await? {
            Some(local) if self.is_dirty(SyncKind::Tag, remote.id).await? => {
                let base = read_base(self.conn, SyncKind::Tag, remote.id)
                    .await?
                    .unwrap_or_else(|| remote.clone());
                let prefer = if remote.updated_at > local.updated_at {
                    Side::Remote
                } else {
                    Side::Local
                };
                TagRecord {
                    id: remote.id,
                    name: merge3_or(&base.name, &local.name, &remote.name, prefer),
                    description: merge3_or(
                        &base.description,
                        &local.description,
                        &remote.description,
                        prefer,
                    ),
                    query: merge3_or(&base.query, &local.query, &remote.query, prefer),
                    auto_delete: merge3_or(
                        &base.auto_delete,
                        &local.auto_delete,
                        &remote.auto_delete,
                        prefer,
                    ),
                    created_at: local.created_at.min(remote.created_at),
                    updated_at: local.updated_at.max(remote.updated_at),
                }
            }
            _ => remote.clone(),
        };
        remove_grave(
            self.conn,
            Grave {
                kind: SyncKind::Tag,
                oid: remote.id,
            },
        )
        .await?;
        let usn = self.usn_for(&merged, remote);
        upsert_tag(self.conn, &merged, usn).await?;
        // Both sides created a tag with this name (a review snapshot, `sync-conflict`, or simply
        // the same new tag). Tag names are unique, so the local one is folded into the server's.
        let clashes: Vec<TagId> =
            sqlx::query_scalar("SELECT id FROM tag WHERE name = ? AND id != ?")
                .bind(&merged.name)
                .bind(merged.id)
                .fetch_all(&mut *self.conn)
                .await
                .map_err(sqlx_error)?;
        for clash in clashes {
            self.fold_tag_into(clash, merged.id).await?;
        }
        Ok(())
    }

    /// Moves everything tagged `from` to `into` and deletes `from`, as a local change.
    async fn fold_tag_into(&mut self, from: TagId, into: TagId) -> Result<(), Error> {
        let note_ids: Vec<i64> =
            sqlx::query_scalar("SELECT note_id FROM note_tag WHERE tag_id = ?")
                .bind(from)
                .fetch_all(&mut *self.conn)
                .await
                .map_err(sqlx_error)?;
        mark_dirty(self.conn, SyncKind::Note, &note_ids).await?;
        let key = from.to_string();
        let card_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT card_id FROM card_tag WHERE tag_id = ?
             UNION SELECT id FROM card
             WHERE json_extract(custom_data, '$.' || json_quote(?)) IS NOT NULL",
        )
        .bind(from)
        .bind(&key)
        .fetch_all(&mut *self.conn)
        .await
        .map_err(sqlx_error)?;
        mark_dirty(self.conn, SyncKind::Card, &card_ids).await?;
        for statement in [
            "INSERT OR IGNORE INTO note_tag (note_id, tag_id)
             SELECT note_id, ?2 FROM note_tag WHERE tag_id = ?1",
            "INSERT OR IGNORE INTO card_tag (card_id, tag_id)
             SELECT card_id, ?2 FROM card_tag WHERE tag_id = ?1",
            // Filtered-tag scheduler data is kept per tag id.
            "UPDATE card SET custom_data = json_set(
                 json_remove(custom_data, '$.' || json_quote(CAST(?1 AS TEXT))),
                 '$.' || json_quote(CAST(?2 AS TEXT)),
                 json_extract(custom_data, '$.' || json_quote(CAST(?1 AS TEXT))))
             WHERE json_extract(custom_data, '$.' || json_quote(CAST(?1 AS TEXT))) IS NOT NULL",
            "UPDATE review_log SET tag_id = ?2, usn = -1 WHERE tag_id = ?1",
        ] {
            sqlx::query(statement)
                .bind(from)
                .bind(into)
                .execute(&mut *self.conn)
                .await
                .map_err(sqlx_error)?;
        }
        delete_row(self.conn, SyncKind::Tag, from).await?;
        insert_grave(
            self.conn,
            Grave {
                kind: SyncKind::Tag,
                oid: from,
            },
            -1,
        )
        .await?;
        self.outcome
            .applied
            .notes_changed
            .extend(note_ids.into_iter().map(NoteId::from_packed));
        Ok(())
    }

    /// The `sync-conflict` tag's id, creating the tag if needed.
    async fn conflict_tag(&mut self) -> Result<TagId, Error> {
        let existing: Option<TagId> = sqlx::query_scalar("SELECT id FROM tag WHERE name = ?")
            .bind(SYNC_CONFLICT_TAG)
            .fetch_optional(&mut *self.conn)
            .await
            .map_err(sqlx_error)?;
        if let Some(id) = existing {
            return Ok(id);
        }
        let id = next_id(self.conn, IdTable::Tag).await?;
        let now = self.now.timestamp();
        upsert_tag(
            self.conn,
            &TagRecord {
                id,
                name: SYNC_CONFLICT_TAG.to_string(),
                description: "Notes that need a look after a sync. Resolve them with `spares sync \
                              conflicts`."
                    .to_string(),
                query: None,
                auto_delete: i64::from(DEFAULT_TAG_AUTO_DELETE),
                created_at: now,
                updated_at: now,
            },
            -1,
        )
        .await?;
        Ok(id)
    }

    /// Records a conflict on `note_id` and tags it, as a local change.
    async fn record_conflict(
        &mut self,
        note_id: NoteId,
        base: Option<&NoteRecord>,
        local: Option<&NoteRecord>,
        remote: Option<&NoteRecord>,
    ) -> Result<(), Error> {
        let json = |record: Option<&NoteRecord>| {
            serde_json::to_string(&record).expect("records serialize")
        };
        sqlx::query(
            "INSERT INTO sync_conflict (note_id, base, local, remote, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(note_id)
        .bind(base.map(|b| json(Some(b))))
        .bind(json(local))
        .bind(json(remote))
        .bind(self.now.timestamp())
        .execute(&mut *self.conn)
        .await
        .map_err(sqlx_error)?;
        let tag_id = self.conflict_tag().await?;
        mark_dirty(self.conn, SyncKind::Note, &[note_id.packed()]).await?;
        sqlx::query("INSERT OR IGNORE INTO note_tag (note_id, tag_id) VALUES (?, ?)")
            .bind(note_id)
            .bind(tag_id)
            .execute(&mut *self.conn)
            .await
            .map_err(sqlx_error)?;
        self.outcome.conflicts.insert(note_id);
        self.outcome.applied.notes_changed.insert(note_id);
        Ok(())
    }

    async fn apply_note_grave(&mut self, note_id: NoteId) -> Result<(), Error> {
        match read_usn(self.conn, SyncKind::Note, note_id.packed()).await? {
            None => Ok(()),
            Some(-1) => {
                // Edited here, deleted there: the edit wins. The server deleted the note's cards
                // and unlinked its reviews, so all of them are sent back.
                let base =
                    read_base::<NoteRecord>(self.conn, SyncKind::Note, note_id.packed()).await?;
                let local = read_note(self.conn, note_id).await?;
                self.record_conflict(note_id, base.as_ref(), local.as_ref(), None)
                    .await?;
                let card_ids: Vec<i64> =
                    sqlx::query_scalar("SELECT id FROM card WHERE note_id = ?")
                        .bind(note_id)
                        .fetch_all(&mut *self.conn)
                        .await
                        .map_err(sqlx_error)?;
                mark_dirty(self.conn, SyncKind::Card, &card_ids).await?;
                sqlx::query(
                    "UPDATE review_log SET usn = -1
                     WHERE card_id IN (SELECT id FROM card WHERE note_id = ?)",
                )
                .bind(note_id)
                .execute(&mut *self.conn)
                .await
                .map_err(sqlx_error)?;
                Ok(())
            }
            Some(_) => {
                delete_row(self.conn, SyncKind::Note, note_id.packed()).await?;
                self.outcome.applied.notes_deleted.insert(note_id);
                Ok(())
            }
        }
    }

    async fn merge_note(&mut self, remote: &NoteRecord) -> Result<(), Error> {
        let Some(local) = read_note(self.conn, remote.id).await? else {
            if has_grave(self.conn, SyncKind::Note, remote.id.packed()).await? {
                // Deleted here, edited there: the edit wins.
                remove_grave(self.conn, Grave::note(remote.id)).await?;
                self.resurrected.insert(remote.id);
                upsert_note(self.conn, remote, self.usn, true).await?;
                self.record_conflict(remote.id, None, None, Some(remote))
                    .await?;
            } else {
                upsert_note(self.conn, remote, self.usn, true).await?;
            }
            self.outcome.applied.notes_changed.insert(remote.id);
            return Ok(());
        };
        if !self.is_dirty(SyncKind::Note, remote.id.packed()).await? {
            if local != *remote {
                upsert_note(self.conn, remote, self.usn, local.data != remote.data).await?;
                self.outcome.applied.notes_changed.insert(remote.id);
            }
            return Ok(());
        }
        let base = read_base::<NoteRecord>(self.conn, SyncKind::Note, remote.id.packed())
            .await?
            // A row that is new on both sides is one this side already pushed, before a sync
            // that did not finish. The local version is the newer one.
            .unwrap_or_else(|| remote.clone());
        let tokens = self.tokenize(remote.id, &base, &local, remote).await?;
        let mut text = match &tokens {
            Some((b, l, r)) => match merge_text(b, l, r) {
                // The sides name different cards in the same place (each replaced a card), but
                // their texts may still merge, matching cards by position.
                TextMerge::Conflict => match merge_text(&base.data, &local.data, &remote.data) {
                    TextMerge::Merged(merged) => TextMerge::Merged(merged),
                    _ => TextMerge::Conflict,
                },
                text => text,
            },
            None => merge_text(&base.data, &local.data, &remote.data),
        };
        if let TextMerge::Merged(ref merged) = text
            && !self.parses(local.parser_id, merged).await?
        {
            text = TextMerge::Conflict;
        }
        // The text of a note whose cards are reconciled is written by `reconcile_cards`.
        let data = match &text {
            TextMerge::Unchanged(Side::Remote) => remote.data.clone(),
            _ => local.data.clone(),
        };
        let merged = NoteRecord {
            id: remote.id,
            data,
            created_at: merge3_or(
                &base.created_at,
                &local.created_at,
                &remote.created_at,
                Side::Local,
            ),
            updated_at: local.updated_at.max(remote.updated_at),
            custom_data: merge_json(
                &base.custom_data,
                &local.custom_data,
                &remote.custom_data,
                Side::Local,
            )
            .0,
            parser_id: merge3_or(
                &base.parser_id,
                &local.parser_id,
                &remote.parser_id,
                Side::Local,
            ),
            tags: merge_sets(&base.tags, &local.tags, &remote.tags),
            keywords: merge_sets(&base.keywords, &local.keywords, &remote.keywords),
        };
        let usn = self.usn_for(&merged, remote);
        upsert_note(self.conn, &merged, usn, merged.data != local.data).await?;
        if merged != local {
            self.outcome.applied.notes_changed.insert(remote.id);
        }
        match text {
            TextMerge::Merged(merged_text) => {
                self.reconcile.insert(remote.id, merged_text);
            }
            TextMerge::Conflict => {
                let local_text = tokens.map_or_else(|| local.data.clone(), |(_, l, _)| l);
                self.reconcile.insert(remote.id, local_text);
                self.record_conflict(remote.id, Some(&base), Some(&local), Some(remote))
                    .await?;
            }
            TextMerge::Unchanged(_) => {}
        }
        Ok(())
    }

    /// Whether `text` (with card ids for orders, see [`orders`]) gives cards, so a merged text
    /// that no longer parses becomes a conflict instead of failing the sync.
    async fn parses(&mut self, parser_id: i64, text: &str) -> Result<bool, Error> {
        let parser = parser_of(self.conn, parser_id).await?;
        let Ok((text, _)) = orders::from_card_ids(parser.as_ref(), text) else {
            return Ok(false);
        };
        let config = read_external_config().ok();
        Ok(add_order_to_note_data(
            parser.as_ref(),
            &text,
            config.as_ref().map(|c| &c.overlapper),
        )
        .is_ok())
    }

    /// The base, local and remote text of a note with each side's card orders replaced by the
    /// ids of the cards they name on that side (see [`orders`]). `None` if the sides use
    /// different parsers, or a text does not parse.
    async fn tokenize(
        &mut self,
        note_id: NoteId,
        base: &NoteRecord,
        local: &NoteRecord,
        remote: &NoteRecord,
    ) -> Result<Option<(String, String, String)>, Error> {
        if base.parser_id != local.parser_id || local.parser_id != remote.parser_id {
            return Ok(None);
        }
        let parser = parser_of(self.conn, local.parser_id).await?;
        let local_cards: HashMap<u32, CardId> =
            sqlx::query_as::<_, (u32, CardId)>(r#"SELECT "order", id FROM card WHERE note_id = ?"#)
                .bind(note_id)
                .fetch_all(&mut *self.conn)
                .await
                .map_err(sqlx_error)?
                .into_iter()
                .collect();
        let remote_cards: HashMap<u32, CardId> = self
            .pulled
            .cards
            .iter()
            .filter(|card| card.note_id == note_id)
            .filter_map(|card| Some((u32::try_from(card.order).ok()?, card.id)))
            .collect();
        // The cards at the last sync: the unchanged ones, and the synced version of those
        // changed or deleted since.
        let base_cards: HashMap<u32, CardId> = sqlx::query_as::<_, (u32, CardId)>(
            r#"SELECT "order", id FROM card WHERE note_id = ?1 AND usn != -1
               UNION ALL
               SELECT json_extract(snapshot, '$.order'), oid FROM sync_base
               WHERE kind = 'card' AND json_extract(snapshot, '$.note_id') = ?1"#,
        )
        .bind(note_id)
        .fetch_all(&mut *self.conn)
        .await
        .map_err(sqlx_error)?
        .into_iter()
        .collect();
        let tokenize = |text: &str, cards: &HashMap<u32, CardId>| {
            orders::to_card_ids(parser.as_ref(), text, cards).ok()
        };
        Ok((|| {
            Some((
                tokenize(&base.data, &base_cards)?,
                tokenize(&local.data, &local_cards)?,
                tokenize(&remote.data, &remote_cards)?,
            ))
        })())
    }

    async fn merge_card(&mut self, remote: &CardRecord) -> Result<(), Error> {
        let Some(local) = read_card(self.conn, remote.id).await? else {
            let card_grave = Grave {
                kind: SyncKind::Card,
                oid: remote.id,
            };
            if has_grave(self.conn, SyncKind::Card, remote.id).await? {
                // A card deleted here (its cloze was removed) stays deleted, unless its whole note
                // comes back.
                if !self.resurrected.contains(&remote.note_id) {
                    return Ok(());
                }
                remove_grave(self.conn, card_grave).await?;
            }
            if upsert_card(self.conn, remote, self.usn).await? {
                self.outcome.applied.notes_changed.insert(remote.note_id);
            }
            return Ok(());
        };
        if !self.is_dirty(SyncKind::Card, remote.id).await? {
            if !cards_equal(&local, remote) {
                upsert_card(self.conn, remote, self.usn).await?;
                self.outcome.applied.notes_changed.insert(remote.note_id);
            }
            return Ok(());
        }
        let base = read_base::<CardRecord>(self.conn, SyncKind::Card, remote.id)
            .await?
            .unwrap_or_else(|| remote.clone());
        let local_latest: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(reviewed_at) FROM review_log WHERE card_id = ? AND usn = -1",
        )
        .bind(remote.id)
        .fetch_one(&mut *self.conn)
        .await
        .map_err(sqlx_error)?;
        let remote_latest = self
            .pulled
            .review_logs
            .iter()
            .filter(|log| log.card_id == Some(remote.id))
            .map(|log| log.reviewed_at)
            .max();
        let (merged, replay) = merge_card(&base, &local, remote, local_latest, remote_latest);
        if replay {
            self.replay.insert(remote.id);
        }
        let usn = if cards_equal(&merged, remote) {
            self.usn
        } else {
            -1
        };
        upsert_card(self.conn, &merged, usn).await?;
        if !cards_equal(&merged, &local) {
            self.outcome.applied.notes_changed.insert(remote.note_id);
        }
        Ok(())
    }

    /// Recomputes the memory state of cards both sides reviewed, from all their reviews.
    async fn replay_cards(&mut self) -> Result<(), Error> {
        let config = read_external_config().unwrap_or_default();
        for card_id in std::mem::take(&mut self.replay) {
            let logs = fetch_review_logs_for_replay(&mut *self.conn, card_id).await?;
            if logs.is_empty() {
                continue;
            }
            let scheduler_name = logs.iter().rev().find(|log| log.is_review()).map_or_else(
                || get_default_scheduler_name().to_string(),
                |log| log.scheduler_name.clone(),
            );
            let scheduler = get_scheduler_from_string(&scheduler_name)?;
            let card: Card = sqlx::query_as("SELECT * FROM card WHERE id = ?")
                .bind(card_id)
                .fetch_one(&mut *self.conn)
                .await
                .map_err(sqlx_error)?;
            let replayed = scheduler.compute_memory_state(&config, logs, card.desired_retention)?;
            sqlx::query(
                "UPDATE card SET stability = ?, difficulty = ?, state = ?, due = ?, usn = -1
                 WHERE id = ?",
            )
            .bind(replayed.stability)
            .bind(replayed.difficulty)
            .bind(replayed.state)
            .bind(replayed.due.timestamp())
            .bind(card_id)
            .execute(&mut *self.conn)
            .await
            .map_err(sqlx_error)?;
        }
        Ok(())
    }

    async fn apply_card_grave(&mut self, card_id: CardId) -> Result<(), Error> {
        let note_id: Option<NoteId> = sqlx::query_scalar("SELECT note_id FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_optional(&mut *self.conn)
            .await
            .map_err(sqlx_error)?;
        let Some(note_id) = note_id else {
            return Ok(());
        };
        // The cards of a note whose text both sides changed follow the merged text.
        if self.reconcile.contains_key(&note_id) {
            return Ok(());
        }
        delete_row(self.conn, SyncKind::Card, card_id).await?;
        self.outcome.applied.notes_changed.insert(note_id);
        Ok(())
    }

    async fn apply_tag_grave(&mut self, tag_id: TagId) -> Result<(), Error> {
        let Some(usn) = read_usn(self.conn, SyncKind::Tag, tag_id).await? else {
            return Ok(());
        };
        let note_ids: Vec<i64> =
            sqlx::query_scalar("SELECT note_id FROM note_tag WHERE tag_id = ?")
                .bind(tag_id)
                .fetch_all(&mut *self.conn)
                .await
                .map_err(sqlx_error)?;
        self.outcome
            .applied
            .notes_changed
            .extend(note_ids.iter().copied().map(NoteId::from_packed));
        if usn == -1 {
            // Changed here: keep the tag, and send back what it is on, which the server dropped
            // with it.
            mark_dirty(self.conn, SyncKind::Note, &note_ids).await?;
            let card_ids: Vec<i64> =
                sqlx::query_scalar("SELECT card_id FROM card_tag WHERE tag_id = ?")
                    .bind(tag_id)
                    .fetch_all(&mut *self.conn)
                    .await
                    .map_err(sqlx_error)?;
            mark_dirty(self.conn, SyncKind::Card, &card_ids).await?;
            return Ok(());
        }
        delete_row(self.conn, SyncKind::Tag, tag_id).await
    }

    async fn apply_parser_grave(&mut self, parser_id: i64) -> Result<(), Error> {
        let Some(usn) = read_usn(self.conn, SyncKind::Parser, parser_id).await? else {
            return Ok(());
        };
        let used: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM note WHERE parser_id = ? LIMIT 1")
                .bind(parser_id)
                .fetch_optional(&mut *self.conn)
                .await
                .map_err(sqlx_error)?;
        if usn == -1 || used.is_some() {
            // Notes here still use it, so the server gets it back.
            mark_dirty(self.conn, SyncKind::Parser, &[parser_id]).await?;
            return Ok(());
        }
        delete_row(self.conn, SyncKind::Parser, parser_id).await
    }
}

fn scheduling_equal(a: &CardRecord, b: &CardRecord) -> bool {
    a.due == b.due
        && a.state == b.state
        && f64_eq(a.stability, b.stability)
        && f64_eq(a.difficulty, b.difficulty)
}

/// Cards compare with a tolerance on their floats, which round-trip through JSON snapshots.
pub(crate) fn cards_equal(a: &CardRecord, b: &CardRecord) -> bool {
    scheduling_equal(a, b)
        && f64_eq(a.desired_retention, b.desired_retention)
        && a.id == b.id
        && a.note_id == b.note_id
        && a.order == b.order
        && a.back_type == b.back_type
        && a.created_at == b.created_at
        && a.special_state == b.special_state
        && a.custom_data == b.custom_data
        && a.tags == b.tags
}

/// Merges a card changed on both sides. Its scheduling (due, stability, difficulty, state) is
/// taken as a whole from one side: the one that changed it, or if both did, the one with the
/// newer review (else the newer change). If both reviewed it, the returned flag asks for the
/// memory state to be replayed from both sides' reviews.
pub(crate) fn merge_card(
    base: &CardRecord,
    local: &CardRecord,
    remote: &CardRecord,
    local_latest_review: Option<i64>,
    remote_latest_review: Option<i64>,
) -> (CardRecord, bool) {
    let newer = if remote.updated_at > local.updated_at {
        Side::Remote
    } else {
        Side::Local
    };
    let local_changed = !scheduling_equal(base, local);
    let remote_changed = !scheduling_equal(base, remote);
    let scheduling_side = if !remote_changed {
        Side::Local
    } else if !local_changed {
        Side::Remote
    } else {
        match (local_latest_review, remote_latest_review) {
            (Some(l), Some(r)) if r > l => Side::Remote,
            (Some(_), _) => Side::Local,
            (None, Some(_)) => Side::Remote,
            (None, None) => newer,
        }
    };
    let replay = local_changed
        && remote_changed
        && local_latest_review.is_some()
        && remote_latest_review.is_some();
    let scheduling = match scheduling_side {
        Side::Local => local,
        Side::Remote => remote,
    };
    let desired_retention = if f64_eq(local.desired_retention, remote.desired_retention)
        || f64_eq(remote.desired_retention, base.desired_retention)
    {
        local.desired_retention
    } else if f64_eq(local.desired_retention, base.desired_retention) {
        remote.desired_retention
    } else {
        match newer {
            Side::Local => local.desired_retention,
            Side::Remote => remote.desired_retention,
        }
    };
    let merged = CardRecord {
        id: local.id,
        note_id: merge3_or(&base.note_id, &local.note_id, &remote.note_id, Side::Local),
        order: merge3_or(&base.order, &local.order, &remote.order, Side::Local),
        back_type: merge3_or(
            &base.back_type,
            &local.back_type,
            &remote.back_type,
            Side::Local,
        ),
        created_at: local.created_at,
        updated_at: local.updated_at.max(remote.updated_at),
        due: scheduling.due,
        stability: scheduling.stability,
        difficulty: scheduling.difficulty,
        desired_retention,
        special_state: merge3_or(
            &base.special_state,
            &local.special_state,
            &remote.special_state,
            newer,
        ),
        state: scheduling.state,
        custom_data: merge_json(
            &base.custom_data,
            &local.custom_data,
            &remote.custom_data,
            newer,
        )
        .0,
        tags: merge_sets(&base.tags, &local.tags, &remote.tags),
    };
    (merged, replay)
}

/// Gives a note whose text both sides changed the cards its final `text` asks for, keeping the
/// card each cloze already has on either side: the card the text names (`text` has card ids for
/// orders, see [`orders`]), else the one with the cloze's uid, else (when the text names no cards
/// at all) the one in its place. Cards nothing asks for are deleted. Writes the text with its card
/// orders renumbered. All of it is a local change, so the server gets the result.
#[expect(clippy::too_many_lines)]
pub(crate) async fn reconcile_cards(
    conn: &mut SqliteConnection,
    note_id: NoteId,
    text: &str,
    now: DateTime<Utc>,
) -> Result<(), Error> {
    let parser_id: i64 = sqlx::query_scalar("SELECT parser_id FROM note WHERE id = ?")
        .bind(note_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    let parser = parser_of(conn, parser_id).await?;
    let config = read_external_config().ok();
    let (text, numbered_cards) = orders::from_card_ids(parser.as_ref(), text)?;
    let (text, card_datas) = add_order_to_note_data(
        parser.as_ref(),
        &text,
        config.as_ref().map(|c| &c.overlapper),
    )?;

    let cards: Vec<Card> =
        sqlx::query_as(r#"SELECT * FROM card WHERE note_id = ? ORDER BY "order", id"#)
            .bind(note_id)
            .fetch_all(&mut *conn)
            .await
            .map_err(sqlx_error)?;
    let uid_of = |card: &Card| {
        card.custom_data
            .get("cloze_uid")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let wanted_uids: BTreeSet<String> = card_datas
        .iter()
        .filter_map(|c| c.cloze_uid.map(|u| u.to_string()))
        .collect();
    let mut by_uid: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, card) in cards.iter().enumerate() {
        if let Some(uid) = uid_of(card) {
            by_uid.entry(uid).or_default().push(i);
        }
    }
    let mut used = vec![false; cards.len()];
    let mut to_create = Vec::new();
    let index_of: HashMap<CardId, usize> = cards
        .iter()
        .enumerate()
        .map(|(j, card)| (card.id, j))
        .collect();
    for (i, card_data) in card_datas.iter().enumerate() {
        let order = (i + 1) as u32;
        // The card the text names, if it still exists.
        let named = card_data
            .previous_order
            .and_then(|previous| numbered_cards.get(&previous))
            .and_then(|id| index_of.get(id).copied())
            .filter(|&j| !used[j]);
        let by_uid_match = || {
            card_data.cloze_uid.and_then(|uid| {
                by_uid
                    .get(&uid.to_string())
                    .and_then(|candidates| candidates.iter().copied().find(|&j| !used[j]))
            })
        };
        // Positional fallback, when the text names no cards at all: a card in the same place
        // whose own cloze is not asked for by uid elsewhere.
        let by_position = || {
            if !numbered_cards.is_empty() {
                return None;
            }
            cards.iter().enumerate().position(|(j, card)| {
                !used[j]
                    && card.order == order
                    && uid_of(card).is_none_or(|uid| !wanted_uids.contains(&uid))
            })
        };
        let picked = named.or_else(by_uid_match).or_else(by_position);
        let Some(j) = picked else {
            let mut card = Card::new(now);
            card.note_id = note_id;
            card.order = order;
            card.back_type = card_data.back_type;
            if card_data.is_suspended == Some(true) {
                card.special_state = Some(SpecialState::Suspended);
            }
            if let (Some(uid), Value::Object(map)) = (card_data.cloze_uid, &mut card.custom_data) {
                map.insert("cloze_uid".to_string(), Value::String(uid.to_string()));
            }
            to_create.push(card);
            continue;
        };
        used[j] = true;
        let card = &cards[j];
        let mut special_state = card.special_state;
        match card_data.is_suspended {
            Some(true) => special_state = Some(SpecialState::Suspended),
            Some(false) if special_state == Some(SpecialState::Suspended) => special_state = None,
            _ => {}
        }
        let mut custom_data = card.custom_data.clone();
        if let Value::Object(ref mut map) = custom_data {
            match card_data.cloze_uid {
                Some(uid) => {
                    map.insert("cloze_uid".to_string(), Value::String(uid.to_string()));
                }
                None => {
                    map.remove("cloze_uid");
                }
            }
        }
        mark_dirty(conn, SyncKind::Card, &[card.id]).await?;
        sqlx::query(
            r#"UPDATE card SET "order" = ?, back_type = ?, special_state = ?, custom_data = ?,
               updated_at = ? WHERE id = ?"#,
        )
        .bind(order)
        .bind(card_data.back_type)
        .bind(special_state)
        .bind(&custom_data)
        .bind(now.timestamp())
        .bind(card.id)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    }
    for (card, used) in cards.iter().zip(&used) {
        if !used {
            delete_row(conn, SyncKind::Card, card.id).await?;
            insert_grave(
                conn,
                Grave {
                    kind: SyncKind::Card,
                    oid: card.id,
                },
                -1,
            )
            .await?;
        }
    }
    create_cards_on(&mut *conn, &to_create).await?;

    if !row_exists(conn, SyncKind::Note, note_id.packed()).await? {
        return Ok(());
    }
    mark_dirty(conn, SyncKind::Note, &[note_id.packed()]).await?;
    let changed = sqlx::query("UPDATE note SET data = ? WHERE id = ? AND data != ?")
        .bind(&text)
        .bind(note_id)
        .bind(&text)
        .execute(&mut *conn)
        .await
        .map_err(sqlx_error)?;
    if changed.rows_affected() > 0 {
        // The renumbered orders can change embedded keywords' or links' positions only, but
        // rebuild them like any other text change.
        let record = read_note(conn, note_id).await?.expect("note exists");
        upsert_note(conn, &record, -1, true).await?;
    }
    Ok(())
}
