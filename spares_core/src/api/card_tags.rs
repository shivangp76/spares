//! Tags on single cards rather than on their notes.
//!
//! A card's own tags are `card_tag` rows of tags without a query. Filtered tags (tags with a
//! query) also live in `card_tag`, but spares assigns those itself, so they can't be added or
//! removed here and are left out of every list of a card's own tags. A search for `tag=name`
//! finds a card when the card or its note has the tag.
//!
//! Changes are logged as an [`EventType::UpdateCardTags`] event, with a `CreateTag` event before
//! it for each tag it created, in one group, as note updates do. Undoing it puts the cards' tags
//! back by name.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use chrono::DateTime;
use chrono::Utc;
use itertools::Itertools;
use serde_json::Value;
use sqlx::AssertSqlSafe;
use sqlx::sqlite::SqliteConnection;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::TagErrorKind;
use crate::api::MAX_ROWS_IN_QUERY;
use crate::api::note::delete_empty_tags_on;
use crate::api::placeholders;
use crate::api::tag::DEFAULT_TAG_AUTO_DELETE;
use crate::api::tag::create_tag_row;
use crate::api::tag::restore_tags;
use crate::api::undo::create_event_group;
use crate::api::undo::insert_events;
use crate::api::undo::join_event_group;
use crate::api::undo::payloads::CardTagsTransition;
use crate::api::undo::payloads::CreateTagPayload;
use crate::api::undo::payloads::DeletedTag;
use crate::api::undo::payloads::Transition;
use crate::api::undo::payloads::UpdateCardTagsPayload;
use crate::helpers::remove_ancestor_tags;
use crate::model::CardId;
use crate::model::EventType;
use crate::model::NoteId;
use crate::model::TagId;
use crate::notes_vcs;
use crate::schema::note::UpdateTags;

/// The own tags of each of `card_ids` (see the module docs), sorted by name. Cards without any
/// are left out.
pub async fn card_tag_names(
    db: &SqlitePool,
    card_ids: &[CardId],
) -> Result<BTreeMap<CardId, Vec<String>>, Error> {
    let mut conn = db.acquire().await.map_err(|e| Error::Sqlx { source: e })?;
    card_tag_names_on(&mut conn, card_ids).await
}

/// [`card_tag_names`] on a connection, such as a transaction's
pub(crate) async fn card_tag_names_on(
    conn: &mut SqliteConnection,
    card_ids: &[CardId],
) -> Result<BTreeMap<CardId, Vec<String>>, Error> {
    let mut names: BTreeMap<CardId, Vec<String>> = BTreeMap::new();
    for chunk in card_ids.chunks(MAX_ROWS_IN_QUERY) {
        let query_str = format!(
            "SELECT ct.card_id, t.name FROM card_tag ct JOIN tag t ON t.id = ct.tag_id
            WHERE ct.card_id IN ({}) AND t.query IS NULL ORDER BY ct.card_id, t.name",
            placeholders(chunk.len())
        );
        let mut query = sqlx::query_as::<_, (CardId, String)>(AssertSqlSafe(query_str));
        for card_id in chunk {
            query = query.bind(card_id);
        }
        for (card_id, name) in query
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| Error::Sqlx { source: e })?
        {
            names.entry(card_id).or_default().push(name);
        }
    }
    Ok(names)
}

/// The names of the tags with a query, which are assigned by spares and can't be given or taken
/// by hand
pub(crate) async fn filtered_tag_names_on(
    conn: &mut SqliteConnection,
) -> Result<Vec<String>, Error> {
    sqlx::query_scalar(r"SELECT name FROM tag WHERE query IS NOT NULL")
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| Error::Sqlx { source: e })
}

fn filtered_tag_error(verb: &str, name: &str) -> Error {
    Error::Library(LibraryError::Tag(TagErrorKind::InvalidInput(format!(
        "Cannot manually {verb} filtered tag `{name}`. Filtered tags are dynamically assigned."
    ))))
}

/// The tags a card with `current` tags has after `update`. Like a note, a card does not get a tag
/// that is an ancestor of one it has, so adding `math` to a card tagged `math:algebra` does
/// nothing.
pub(crate) fn updated_tag_names(
    current: &[String],
    update: &UpdateTags,
    filtered_tag_names: &[String],
) -> Result<Vec<String>, Error> {
    let after: Vec<String> = match update {
        UpdateTags::None => return Ok(current.to_vec()),
        UpdateTags::SetTags(tags) => tags.clone(),
        UpdateTags::ModifyTags {
            tags_to_remove,
            tags_to_add,
        } => {
            let to_remove = tags_to_remove.as_deref().unwrap_or_default();
            if let Some(name) = to_remove.iter().find(|t| filtered_tag_names.contains(t)) {
                return Err(filtered_tag_error("remove", name));
            }
            current
                .iter()
                .filter(|tag| !to_remove.contains(tag))
                .chain(tags_to_add.iter().flatten())
                .cloned()
                .collect()
        }
    };
    if let Some(name) = after.iter().find(|t| filtered_tag_names.contains(t)) {
        return Err(filtered_tag_error("add", name));
    }
    let mut after = remove_ancestor_tags(&after.into_iter().unique().collect::<Vec<_>>());
    after.sort();
    Ok(after)
}

/// The tags creating and deleting that changing cards' tags did, for its undo events
#[derive(Debug, Default)]
pub(crate) struct CardTagChanges {
    pub cards: Vec<CardTagsTransition>,
    pub note_ids: BTreeSet<NoteId>,
    pub created: Vec<CreateTagPayload>,
    pub deleted: Vec<DeletedTag>,
}

impl CardTagChanges {
    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }
}

/// Gives card `card_id` exactly the own tags `names` (sorted, without ancestors of each other or
/// filtered tags), creating the tags that don't exist and deleting the
/// [`auto_delete`](crate::model::Tag::auto_delete) tags no note or card has anymore. Its filtered
/// tags are kept. Returns the tags it created and deleted.
async fn set_card_tags_on(
    conn: &mut SqliteConnection,
    card_id: CardId,
    names: &[String],
) -> Result<(Vec<CreateTagPayload>, Vec<DeletedTag>), Error> {
    let current: Vec<(TagId, String, bool)> = sqlx::query_as(
        r"SELECT t.id, t.name, t.auto_delete FROM card_tag ct JOIN tag t ON t.id = ct.tag_id
        WHERE ct.card_id = ? AND t.query IS NULL",
    )
    .bind(card_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let removed: Vec<&(TagId, String, bool)> = current
        .iter()
        .filter(|(_, name, _)| !names.contains(name))
        .collect();
    for (tag_id, _, _) in &removed {
        sqlx::query(r"DELETE FROM card_tag WHERE card_id = ? AND tag_id = ?")
            .bind(card_id)
            .bind(tag_id)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }
    let mut created = Vec::new();
    for name in names
        .iter()
        .filter(|name| !current.iter().any(|(_, current, _)| current == *name))
    {
        // There is no unique constraint on `tag.name`, so the oldest tag of a name is used, as
        // when notes are tagged
        let existing: Option<TagId> =
            sqlx::query_scalar(r"SELECT id FROM tag WHERE name = ? ORDER BY id LIMIT 1")
                .bind(name)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        let tag_id = if let Some(tag_id) = existing {
            tag_id
        } else {
            let tag = create_tag_row(&mut *conn, name).await?;
            created.push(CreateTagPayload {
                id: Some(tag.id),
                name: name.clone(),
                description: String::new(),
                query: None,
                auto_delete: DEFAULT_TAG_AUTO_DELETE,
                reschedule: true,
                created_at: tag.created_at,
                updated_at: tag.updated_at,
                note_ids: vec![],
                card_ids: vec![],
                review_log_ids: vec![],
            });
            tag.id
        };
        sqlx::query(r"INSERT OR IGNORE INTO card_tag (card_id, tag_id) VALUES (?, ?)")
            .bind(card_id)
            .bind(tag_id)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }
    let emptied: Vec<TagId> = removed
        .iter()
        .filter(|(_, _, auto_delete)| *auto_delete)
        .map(|(tag_id, _, _)| *tag_id)
        .collect();
    let deleted = delete_empty_tags_on(&mut *conn, &emptied).await?;
    Ok((created, deleted))
}

/// Sets the own tags of cards to `tags` (card id, tag names), returning what changed. Cards that
/// no longer exist and cards whose tags are already `tags` are skipped. The names must already be
/// checked, e.g. by [`updated_tag_names`].
pub(crate) async fn set_card_tags(
    conn: &mut SqliteConnection,
    tags: &[(CardId, Vec<String>)],
) -> Result<CardTagChanges, Error> {
    let card_ids: Vec<CardId> = tags.iter().map(|(card_id, _)| *card_id).collect();
    let mut current = card_tag_names_on(&mut *conn, &card_ids).await?;
    let mut note_ids_by_card: BTreeMap<CardId, NoteId> = BTreeMap::new();
    for chunk in card_ids.chunks(MAX_ROWS_IN_QUERY) {
        let query_str = format!(
            "SELECT id, note_id FROM card WHERE id IN ({})",
            placeholders(chunk.len())
        );
        let mut query = sqlx::query_as::<_, (CardId, NoteId)>(AssertSqlSafe(query_str));
        for card_id in chunk {
            query = query.bind(card_id);
        }
        note_ids_by_card.extend(
            query
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| Error::Sqlx { source: e })?,
        );
    }
    let mut changes = CardTagChanges::default();
    for (card_id, after) in tags {
        let Some(note_id) = note_ids_by_card.get(card_id) else {
            continue;
        };
        let before = current.remove(card_id).unwrap_or_default();
        if before == *after {
            continue;
        }
        let (created, deleted) = set_card_tags_on(&mut *conn, *card_id, after).await?;
        changes.created.extend(created);
        changes.deleted.extend(deleted);
        changes.note_ids.insert(*note_id);
        changes.cards.push(CardTagsTransition {
            card_id: *card_id,
            tags: Transition {
                before,
                after: after.clone(),
            },
        });
    }
    Ok(changes)
}

/// Applies `update` to the own tags of each of `card_ids`, in one transaction. See
/// [`updated_tag_names`].
pub(crate) async fn update_card_tags_on(
    conn: &mut SqliteConnection,
    card_ids: &[CardId],
    update: &UpdateTags,
) -> Result<CardTagChanges, Error> {
    if matches!(update, UpdateTags::None) || card_ids.is_empty() {
        return Ok(CardTagChanges::default());
    }
    let filtered = filtered_tag_names_on(&mut *conn).await?;
    let current = card_tag_names_on(&mut *conn, card_ids).await?;
    let tags = card_ids
        .iter()
        .map(|card_id| {
            let current = current.get(card_id).map_or(&[][..], Vec::as_slice);
            Ok((*card_id, updated_tag_names(current, update, &filtered)?))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    set_card_tags(conn, &tags).await
}

/// Logs `changes` as an [`EventType::UpdateCardTags`] event, after a `CreateTag` event for each
/// tag they created, in one group. With `group`, they join that group instead, after its events.
/// Returns the event that leads them, or `None` if nothing changed.
pub(crate) async fn log_card_tag_changes(
    db: &SqlitePool,
    changes: CardTagChanges,
    at: DateTime<Utc>,
    group: Option<i64>,
) -> Result<Option<i64>, Error> {
    if changes.is_empty() {
        return Ok(None);
    }
    let to_value = |value: Result<Value, serde_json::Error>| {
        value.map_err(|e| {
            Error::Library(LibraryError::InvalidConfig(format!(
                "failed to serialize card tags payload: {e}"
            )))
        })
    };
    let payload = UpdateCardTagsPayload {
        cards: changes.cards,
        note_ids: changes.note_ids.into_iter().collect(),
        deleted_tags: changes.deleted,
        restored_tags: Vec::new(),
    };
    let mut events = changes
        .created
        .iter()
        .map(|tag| Ok((EventType::CreateTag, to_value(serde_json::to_value(tag))?)))
        .collect::<Result<Vec<_>, Error>>()?;
    events.push((
        EventType::UpdateCardTags,
        to_value(serde_json::to_value(&payload))?,
    ));
    if let Some(leader) = group {
        for event in events {
            let ids = insert_events(db, &[event], at, None).await?;
            for id in ids {
                join_event_group(db, leader, id).await?;
            }
        }
        return Ok(Some(leader));
    }
    if events.len() == 1 {
        let ids = insert_events(db, &events, at, None).await?;
        return Ok(ids.first().copied());
    }
    Ok(Some(create_event_group(db, events, at).await?))
}

/// Applies an `UpdateCardTags` payload, giving each card its `after` tags: an undo's or redo's,
/// whose `create_undo_event` swapped them. Tags it needs that are gone are restored from
/// `restored_tags`, or created again by name, unlogged.
pub async fn update_card_tags_event(
    db: &SqlitePool,
    payload: UpdateCardTagsPayload,
) -> Result<(), Error> {
    restore_tags(db, &payload.restored_tags).await?;
    let mut tx = db.begin().await.map_err(|e| Error::Sqlx { source: e })?;
    let tags: Vec<(CardId, Vec<String>)> = payload
        .cards
        .into_iter()
        .map(|card| (card.card_id, card.tags.after))
        .collect();
    set_card_tags(&mut tx, &tags).await?;
    tx.commit().await.map_err(|e| Error::Sqlx { source: e })?;
    Ok(())
}

/// Changes the own tags of `card_ids` as `update` says, logged as an
/// [`EventType::UpdateCardTags`] event if `log`. The notes of the cards that changed have their
/// files rewritten in the notes repository. Returns the event, if one was logged.
pub async fn update_card_tags(
    db: &SqlitePool,
    card_ids: &[CardId],
    update: &UpdateTags,
    at: DateTime<Utc>,
    log: bool,
) -> Result<Option<i64>, Error> {
    let mut tx = db.begin().await.map_err(|e| Error::Sqlx { source: e })?;
    let changes = update_card_tags_on(&mut tx, card_ids, update).await?;
    tx.commit().await.map_err(|e| Error::Sqlx { source: e })?;
    let note_ids: Vec<NoteId> = changes.note_ids.iter().copied().collect();
    let event_id = if log {
        log_card_tag_changes(db, changes, at, None).await?
    } else {
        None
    };
    notes_vcs::reconcile_after(db, note_ids, "tag cards").await;
    Ok(event_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(tags: &[&str]) -> Vec<String> {
        tags.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn updated_tag_names_adds_removes_and_drops_ancestors() {
        let current = names(&["a", "math:algebra"]);
        let update = UpdateTags::ModifyTags {
            tags_to_remove: Some(names(&["a"])),
            tags_to_add: Some(names(&["math", "b", "b"])),
        };
        assert_eq!(
            updated_tag_names(&current, &update, &[]).unwrap(),
            names(&["b", "math:algebra"])
        );
        assert_eq!(
            updated_tag_names(&current, &UpdateTags::SetTags(vec![]), &[]).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            updated_tag_names(&current, &UpdateTags::None, &[]).unwrap(),
            current
        );
    }

    #[test]
    fn updated_tag_names_refuses_filtered_tags() {
        let filtered = names(&["exam"]);
        let add = UpdateTags::ModifyTags {
            tags_to_remove: None,
            tags_to_add: Some(names(&["exam"])),
        };
        assert!(updated_tag_names(&[], &add, &filtered).is_err());
        let remove = UpdateTags::ModifyTags {
            tags_to_remove: Some(names(&["exam"])),
            tags_to_add: None,
        };
        assert!(updated_tag_names(&[], &remove, &filtered).is_err());
    }
}
