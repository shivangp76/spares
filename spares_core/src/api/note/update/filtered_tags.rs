use std::collections::HashSet;

use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::api::card::create_card_tags;
use crate::api::card::delete_card_tags;
use crate::api::fetch_batched_query;
use crate::api::placeholders;
use crate::model::CardId;
use crate::model::TagId;
use crate::schema::note::NoteResponse;
use crate::search::evaluator::Evaluator;

/// Re-evaluates all filtered-tag queries against the given notes' cards, adding or removing
/// card-tag associations as appropriate.  Must be called after cards and manual tags are committed.
pub(super) async fn rebuild_filtered_tags_for_updated_notes(
    db: &SqlitePool,
    note_responses: &[NoteResponse],
) -> Result<(), Error> {
    let existing_filtered_tags: Vec<(TagId, String)> =
        sqlx::query_as(r"SELECT id, query FROM tag WHERE query IS NOT NULL")
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    let created_card_ids: Vec<CardId> =
        fetch_batched_query(db, note_responses, async |db, chunk| {
            let query_str = format!(
                "SELECT id FROM cards WHERE note_id IN ({})",
                placeholders(chunk.len())
            );
            let mut query = sqlx::query_scalar(query_str.as_str());
            for note in chunk {
                query = query.bind(note.id);
            }
            query
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })
        })
        .await?;
    let mut card_filtered_tag_entries = Vec::new();
    let mut delete_card_tag_entries = Vec::new();
    for (tag_id, query) in existing_filtered_tags {
        let evaluator = Evaluator::new(query.as_str());
        let search_card_ids = evaluator.get_card_ids(db).await?;
        let (card_ids_to_add_tag, card_ids_to_remove_tag): (Vec<_>, Vec<_>) = created_card_ids
            .iter()
            .map(|card_id| (*card_id, tag_id))
            .partition(|(card_id, _)| search_card_ids.contains(card_id));
        let existing_card_tags: Vec<(CardId, TagId)> =
            fetch_batched_query(db, &created_card_ids, async |db, chunk| {
                let query_str = format!(
                    "SELECT card_id, tag_id FROM card_tag WHERE card_id IN ({}) AND tag_id = ?",
                    placeholders(chunk.len())
                );
                let mut query = sqlx::query_as(query_str.as_str());
                for card_id in chunk {
                    query = query.bind(card_id);
                }
                query
                    .bind(tag_id)
                    .fetch_all(db)
                    .await
                    .map_err(|e| Error::Sqlx { source: e })
            })
            .await?;
        let existing_card_tags_set: HashSet<(CardId, TagId)> =
            existing_card_tags.into_iter().collect();
        let card_ids_to_add_tag: Vec<(CardId, TagId)> = card_ids_to_add_tag
            .into_iter()
            .filter(|entry| !existing_card_tags_set.contains(entry))
            .collect();
        card_filtered_tag_entries.extend(card_ids_to_add_tag);
        delete_card_tag_entries.extend(card_ids_to_remove_tag);
    }
    create_card_tags(db, &card_filtered_tag_entries).await?;
    delete_card_tags(db, &delete_card_tag_entries).await
}
