use std::collections::HashMap;

use sqlx::sqlite::SqliteConnection;

use super::super::create_note_links_on;
use crate::Error;
use crate::model::NoteId;
use crate::model::NoteLink;
use crate::parsers::Parseable;

pub(crate) async fn update_note_links(
    conn: &mut SqliteConnection,
    note_id: NoteId,
    new_parser: &dyn Parseable,
    new_data: &str,
) -> Result<(), Error> {
    // Get old linked notes from note_link table
    let old_note_links: Vec<NoteLink> =
        sqlx::query_as(r#"SELECT * FROM note_link WHERE parent_note_id = ? ORDER BY "order""#)
            .bind(note_id)
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

    // Reparse linked notes from new note data
    let new_links = new_parser
        .get_linked_notes_with_relations(new_data)
        .map_err(Error::Library)?;

    // Create a hashmap from old note links: searched_keyword -> (matched_keyword, linked_note_id, score)
    let old_note_links_map: HashMap<String, NoteLink> = old_note_links
        .iter()
        .map(|nl| (nl.searched_keyword.clone(), nl.clone()))
        .collect();

    // Check if old and new linked notes match up exactly by (order, searched_keyword, relation)
    let links_match_exactly = old_note_links.len() == new_links.len()
        && old_note_links.iter().zip(&new_links).all(|(old, new)| {
            old.searched_keyword == new_data[new.range.clone()] && old.relation == new.relation
        });

    if !links_match_exactly {
        // Delete all linked notes for this note
        let _delete_result = sqlx::query(r"DELETE FROM note_link WHERE parent_note_id = ?")
            .bind(note_id)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

        // Create new note links, preserving matched info where possible
        let new_note_links: Vec<NoteLink> = new_links
            .into_iter()
            .enumerate()
            .map(|(i, link)| {
                let searched_keyword = new_data[link.range].to_string();
                // Try to find matching info from old note links by searched_keyword
                let nl_opt = old_note_links_map.get(&searched_keyword);

                NoteLink {
                    parent_note_id: note_id,
                    linked_note_id: nl_opt.map(|nl| nl.linked_note_id).unwrap_or_default(),
                    order: i as u32,
                    searched_keyword,
                    matched_keyword: nl_opt
                        .map(|nl| nl.matched_keyword.clone())
                        .unwrap_or_default(),
                    score: nl_opt.map(|nl| nl.score).unwrap_or_default(),
                    relation: link.relation,
                }
            })
            .collect();

        // Insert all new linked note ids
        if !new_note_links.is_empty() {
            create_note_links_on(&mut *conn, &new_note_links).await?;
        }
    }

    Ok(())
}
