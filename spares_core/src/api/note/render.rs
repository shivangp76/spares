use std::collections::HashMap;
use std::collections::HashSet;

// use indicatif::ParallelProgressIterator;
use itertools::Itertools;
use rayon::prelude::*;
use serde_json::Value;
use sqlx::FromRow;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::api::MAX_ROWS_IN_QUERY;
use crate::api::execute_batched_query;
use crate::api::note::create_note_links;
use crate::api::note::get_keywords;
use crate::api::note::keyword_distance::weighted_levenshtein;
use crate::api::placeholders;
use crate::api::placeholders_2d;
use crate::config::read_internal_config;
use crate::config::write_internal_config;
use crate::helpers::value_to_string_vec;
use crate::model::NoteId;
use crate::model::NoteLink;
use crate::parsers::Parseable;
use crate::parsers::find_parser;
use crate::parsers::generate_files::GenerateNoteFilesRequest;
use crate::parsers::generate_files::GenerateNoteFilesRequests;
use crate::parsers::generate_files::create_note_files_bulk;
use crate::schema::note::LinkedNote;
use crate::schema::note::MatchedKeywordResponse;
use crate::schema::note::NotesSelector;
use crate::schema::note::RenderNotesRequest;
use crate::search::evaluator::Evaluator;

#[derive(Debug, FromRow)]
pub struct RenderNoteData {
    pub note_id: NoteId,
    pub data: String,
    pub keywords_value: Value,
    pub custom_data: Value,
    pub parser_name: String,
    pub tags_value: Value,
}

pub fn match_keyword(
    searched_keyword: &str,
    keywords: &[(NoteId, String)],
) -> Vec<MatchedKeywordResponse> {
    let searched_keyword_lower = searched_keyword.to_ascii_lowercase();
    keywords
        .par_iter()
        .filter_map(|(id, keyword)| {
            weighted_levenshtein(
                searched_keyword_lower.as_str(),
                keyword.to_ascii_lowercase().as_str(),
            )
            .map(|score| ((id, keyword), score))
        })
        .map(
            |((note_id, matched_keyword), score)| MatchedKeywordResponse {
                matched_keyword: matched_keyword.clone(),
                note_id: *note_id,
                score,
            },
        )
        .collect::<Vec<_>>()
}

pub async fn get_render_note_data(
    db: &SqlitePool,
    requested_note_ids: Option<Vec<NoteId>>,
) -> Result<Vec<RenderNoteData>, Error> {
    let query_str = format!(
        r"SELECT
          n.id as note_id,
          n.data,
          COALESCE((SELECT JSON_GROUP_ARRAY(nk.keyword)
           FROM note_keyword nk
           WHERE nk.note_id = n.id AND nk.embedded = 0), '[]') as keywords_value,
          n.custom_data,
          p.name as parser_name,
          COALESCE(JSON_GROUP_ARRAY(t.name), '[]') AS tags_value
        FROM
          note n
        LEFT JOIN
          note_tag nt ON n.id = nt.note_id
        LEFT JOIN
          tag t ON t.id = nt.tag_id AND t.query IS NULL
        LEFT JOIN
          parser p ON n.parser_id = p.id
        {}
        GROUP BY
          n.id
        ORDER BY
          n.id",
        requested_note_ids
            .as_ref()
            .filter(|ids| !ids.is_empty())
            .map_or(String::new(), |ids| format!(
                "WHERE n.id IN ({})",
                placeholders(ids.len())
            ))
    );
    let mut query = sqlx::query_as(&query_str);
    if let Some(ref note_ids) = requested_note_ids {
        for note_id in note_ids {
            query = query.bind(note_id);
        }
    }
    let notes_data: Vec<RenderNoteData> = query
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(notes_data)
}

/// Updates existing note links by re-matching them against keywords.
/// Returns the updated note links
async fn update_existing_note_links(db: &SqlitePool) -> Result<Vec<NoteLink>, Error> {
    // Get all keywords
    let keywords = get_keywords(db).await?;

    // Get all existing note links where score != 0
    // The second case is for when a note is deleted. Any notes that used to link to that deleted note need to be updated as well.
    let existing_note_links: Vec<NoteLink> =
        sqlx::query_as(r"SELECT * FROM note_link WHERE (score IS NULL OR score != 0) OR (score IS NOT NULL AND linked_note_id IS NULL)")
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

    // Re-match existing linked notes against keywords
    let updated_note_links: Vec<NoteLink> = existing_note_links
        .par_iter()
        .filter_map(|existing_link| {
            let matched_keyword_data =
                match_keyword(&existing_link.searched_keyword, keywords.as_ref())
                    .into_iter()
                    .min_by(|a, b| {
                        a.score
                            .partial_cmp(&b.score)
                            // ignore NaN and always rank it last (i.e., treat NaN as largest)
                            .unwrap_or(std::cmp::Ordering::Greater)
                    });
            if matched_keyword_data.as_ref().map(|x| x.score) == existing_link.score
                && (matched_keyword_data
                    .as_ref()
                    .map(|x| x.matched_keyword.clone())
                    == existing_link.matched_keyword)
                && (matched_keyword_data.as_ref().map(|x| x.note_id)
                    == existing_link.linked_note_id)
            {
                return None;
            }
            Some(NoteLink {
                parent_note_id: existing_link.parent_note_id,
                linked_note_id: matched_keyword_data.as_ref().map(|x| x.note_id),
                order: existing_link.order,
                searched_keyword: existing_link.searched_keyword.clone(),
                matched_keyword: matched_keyword_data
                    .as_ref()
                    .map(|x| x.matched_keyword.clone()),
                score: matched_keyword_data.as_ref().map(|x| x.score),
            })
        })
        .collect();
    Ok(updated_note_links)
}

/// Deletes the old rows for the given note links and re-inserts the updated ones.
async fn persist_updated_note_links(
    db: &SqlitePool,
    updated_note_links: &[NoteLink],
) -> Result<(), Error> {
    execute_batched_query(
        db,
        updated_note_links,
        MAX_ROWS_IN_QUERY,
        async |db, chunk| {
            let query_str = format!(
                "DELETE FROM note_link WHERE (parent_note_id, \"order\") IN ({})",
                placeholders_2d(chunk.len(), 2)
            );
            let mut delete_query = sqlx::query(query_str.as_str());
            for nl in chunk {
                delete_query = delete_query.bind(nl.parent_note_id);
                delete_query = delete_query.bind(nl.order);
            }
            delete_query
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
            Ok(())
        },
    )
    .await?;
    create_note_links(db, updated_note_links).await
}

/// - Determines linked notes for _all_ notes. This is not possible for only some notes. See note below.
/// - Generates files for specified notes, usually all notes.
///
/// Note: Only generating linked notes for some notes is not possible. Suppose a user has 3 notes: Notes A, B, and C. Suppose the user requests Note A to be rendered. Suppose Note B currently has a keyword that matches with Note C. However, the change to Note A could mean that Note B now has a better match with Note A. This means that Note B should be rendered as well. Therefore, it is possible that notes that are not requested need to have their linked notes regenerated as well.
#[expect(clippy::too_many_lines)]
pub async fn render_notes(
    db: &SqlitePool,
    body: RenderNotesRequest,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<(), Error> {
    let RenderNotesRequest {
        selector: generate_files_note_ids,
        immutable_note_ids,
        overridden_output_raw_dir,
        include_linked_notes,
        include_cards,
        generate_rendered,
        force_generate_rendered,
    } = body;

    let mut changed_note_ids: HashSet<NoteId> = HashSet::new();
    if include_linked_notes {
        // Match linked notes to keyword
        let updated_note_links = update_existing_note_links(db).await?;
        changed_note_ids = updated_note_links
            .iter()
            .map(|l| l.parent_note_id)
            .collect();

        // Update note links with updated score
        persist_updated_note_links(db, &updated_note_links).await?;
        // Note: We'll load all note links for rendered notes later, after we know which notes we're rendering
        // The updated note links are already in the database, so we'll get them when we query
    }

    // Update config
    let mut config = read_internal_config(db).await?;
    config.linked_notes_generated = true;
    write_internal_config(db, &config).await?;

    // Get requested note ids
    let mut requested_note_ids = match generate_files_note_ids {
        NotesSelector::Query(query) => {
            let evaluator = Evaluator::new(&query);
            Some(
                evaluator
                    .get_note_ids(db)
                    .await?
                    .into_iter()
                    .collect::<HashSet<_>>(),
            )
        }
        NotesSelector::Ids(vec) => Some(vec.into_iter().collect::<HashSet<_>>()),
        NotesSelector::All => None,
    };
    // Extend requested note ids to include notes that had their note links changed and are not immutable
    if let Some(ref mut note_ids) = requested_note_ids
        && let Some(immutable_note_ids) = immutable_note_ids
    {
        note_ids.extend(
            changed_note_ids
                .difference(&immutable_note_ids.iter().copied().collect::<HashSet<_>>()),
        );
    }
    if let Some(ref note_ids) = requested_note_ids
        && note_ids.is_empty()
    {
        // No notes should be regenerated, so we are done.
        return Ok(());
    }

    // Render requested note ids
    //
    // Get notes data. Note that some other notes may have had their linked notes match to another note. However, we do not render them if their note id is not requested. (Unless all notes are requested.)
    let notes_data = get_render_note_data(
        db,
        requested_note_ids.map(|x| x.into_iter().collect::<Vec<_>>()),
    )
    .await?;

    // Load all note links for the notes we're rendering (if include_linked_notes)
    let mut linked_notes_map: Option<HashMap<_, _>> = None;
    if include_linked_notes {
        let note_ids_for_links: Vec<NoteId> = notes_data.iter().map(|n| n.note_id).collect();
        // Note that the query sorts by order, so we don't need to do this after
        let query_str = format!(
            "SELECT * FROM note_link WHERE parent_note_id IN ({}) ORDER BY parent_note_id, \"order\"",
            placeholders(note_ids_for_links.len())
        );
        let mut query = sqlx::query_as(&query_str);
        for note_id in &note_ids_for_links {
            query = query.bind(note_id);
        }
        let all_note_links: Vec<NoteLink> = query
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

        // Build linked_notes_map with all note links for rendered notes
        let mut all_note_links_map: HashMap<NoteId, Vec<NoteLink>> = HashMap::new();
        for note_link in all_note_links {
            all_note_links_map
                .entry(note_link.parent_note_id)
                .or_default()
                .push(note_link);
        }
        linked_notes_map = Some(all_note_links_map);
    }

    // Generate files for notes and cards
    // This must be done after linking because the links need to be shown in the rendered note.
    //
    // Group notes by parser
    let grouped_parse_note_requests = notes_data
        .iter()
        .map(|render_note_data| {
            (
                &render_note_data.parser_name,
                render_note_data_to_generate_files_request(
                    render_note_data,
                    linked_notes_map.as_ref(),
                ),
            )
        })
        .into_group_map();
    for (parser_name, generate_note_files_request) in grouped_parse_note_requests {
        let parser = find_parser(parser_name, all_parsers)?;
        let generate_note_files_requests = GenerateNoteFilesRequests {
            requests: generate_note_files_request,
            overridden_output_raw_dir: overridden_output_raw_dir.clone(),
            include_cards,
            render: generate_rendered,
            force_render: force_generate_rendered,
            precomputed_cards: None,
        };
        let _card_paths = create_note_files_bulk(parser.as_ref(), &generate_note_files_requests)?
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
    }
    Ok(())
}

pub fn render_note_data_to_generate_files_request<S: ::std::hash::BuildHasher>(
    render_note_data: &RenderNoteData,
    linked_notes_map: Option<&HashMap<NoteId, Vec<NoteLink>, S>>,
) -> GenerateNoteFilesRequest {
    let RenderNoteData {
        note_id,
        data,
        keywords_value,
        custom_data,
        parser_name: _,
        tags_value,
    } = render_note_data;
    let linked_notes = linked_notes_map.as_ref().map(|mapping| {
        mapping.get(note_id).map(|note_links| {
            note_links
                .iter()
                .map(
                    |NoteLink {
                         searched_keyword,
                         linked_note_id,
                         matched_keyword,
                         ..
                     }| LinkedNote {
                        searched_keyword: searched_keyword.clone(),
                        linked_note_id: *linked_note_id,
                        matched_keyword: matched_keyword.clone(),
                    },
                )
                .collect::<Vec<_>>()
        })
    });
    // Parse JSON arrays into Vec<String>
    let keywords: Vec<String> = value_to_string_vec(keywords_value);
    let mut tags: Vec<String> = value_to_string_vec(tags_value);
    tags.sort();
    GenerateNoteFilesRequest {
        note_id: *note_id,
        note_data: data.clone(),
        keywords,
        linked_notes: linked_notes.flatten(),
        custom_data: custom_data.as_object().unwrap().clone(),
        tags,
    }
}

#[cfg(test)]
mod tests {
    use sqlx::SqlitePool;

    use crate::api::note::basic::tests::create_note_helper;
    use crate::api::note::render_notes;
    use crate::model::NoteLink;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::NotesSelector;
    use crate::schema::note::RenderNotesRequest;

    #[sqlx::test]
    async fn test_render_note(pool: SqlitePool) -> () {
        let _ = create_note_helper(&pool).await;
        let body = RenderNotesRequest {
            selector: NotesSelector::All,
            immutable_note_ids: None,
            overridden_output_raw_dir: None,
            include_linked_notes: true,
            include_cards: true,
            generate_rendered: false,
            force_generate_rendered: false,
        };
        let res = render_notes(&pool, body, &get_all_parsers()).await;
        assert!(res.is_ok());

        let note_links: Result<Vec<NoteLink>, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM note_link")
                .fetch_all(&pool)
                .await;
        assert!(note_links.is_ok());
        let note_links = note_links.unwrap();
        assert_eq!(note_links.len(), 3);
        assert_eq!(
            note_links
                .iter()
                .map(|nl| nl.searched_keyword.clone())
                .collect::<Vec<_>>(),
            vec!["keyword 1", "keywords 1", "keyword 2"]
        );
        assert_eq!(
            note_links
                .iter()
                .map(|nl| nl.matched_keyword.clone())
                .collect::<Vec<_>>(),
            vec![
                Some("keyword 1".to_string()),
                Some("keyword 1".to_string()),
                Some("keyword 2".to_string())
            ]
        );
    }
}
