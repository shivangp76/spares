use crate::{
    Error,
    api::note::get_keywords,
    config::{read_internal_config, write_internal_config},
    helpers::parse_list,
    model::{NoteId, NoteLink},
    parsers::{
        Parseable, find_parser,
        generate_files::{
            GenerateNoteFilesRequest, GenerateNoteFilesRequests, create_note_files_bulk,
        },
    },
    schema::note::{GenerateFilesNoteIds, LinkedNote, RenderNotesRequest},
    search::evaluator::Evaluator,
};
// use indicatif::ParallelProgressIterator;
use itertools::Itertools;
use rayon::prelude::*;
use serde_json::Value;
use sqlx::FromRow;
use sqlx::sqlite::SqlitePool;
use std::collections::HashMap;

pub const MAX_KEYWORD_DIFFERENCE_SCORE: usize = 10;

#[derive(Debug, FromRow)]
struct RenderNoteData {
    pub note_id: NoteId,
    pub data: String,
    pub keywords: String,
    pub custom_data: Value,
    pub parser_name: String,
    pub tags_str: String,
}

async fn match_keyword_to_linked_note(
    db: &SqlitePool,
    notes_data_grouped: HashMap<&str, Vec<&RenderNoteData>>,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<Vec<NoteLink>, Error> {
    // Get all keywords
    let keywords = get_keywords(db).await?;

    // Get all linked notes
    let linked_notes_ids = notes_data_grouped
        // .par_iter()
        // .progress_count(notes_data.len() as u64)
        .iter()
        .map(|(parser_name, notes_data)| -> Result<_, Error> {
            let parser = find_parser(parser_name, all_parsers)?;
            let data = notes_data
                .iter()
                .map(|render_notes_data| {
                    let RenderNoteData { note_id, data, .. } = render_notes_data;
                    let note_links = parser
                        .get_linked_notes(data)?
                        .into_par_iter()
                        .enumerate()
                        .map(|(i, searched_keyword_range)| {
                            let searched_keyword = &data[searched_keyword_range];
                            let matched_keyword_data = keywords
                                .par_iter()
                                .min_by_key(|(_id, keyword)| {
                                    // Do not return matches that are too far apart
                                    Some(strsim::levenshtein(searched_keyword, keyword))
                                        .filter(|&score| score <= MAX_KEYWORD_DIFFERENCE_SCORE);
                                })
                                .cloned();
                            NoteLink {
                                // id: None,
                                parent_note_id: *note_id,
                                linked_note_id: matched_keyword_data.as_ref().map(|x| x.0),
                                order: i as u32,
                                searched_keyword: searched_keyword.to_owned(),
                                matched_keyword: matched_keyword_data.as_ref().map(|x| x.1.clone()),
                            }
                        })
                        .collect::<Vec<_>>();
                    Ok(note_links)
                })
                .collect::<Result<Vec<_>, Error>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            Ok(data)
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    Ok(linked_notes_ids)
}

/// - Determines linked notes for _all_ notes. This is not possible for all notes. See note below.
/// - Generates files for specified notes, usually all notes.
///
/// Note: Only generating linked notes for some notes is not possible. Suppose a user has 3 notes: Notes A, B, and C. Suppose the user requests Note A to be rendered. Suppose Note B currently has a keyword that matches with Note C. However, the change to Note A could mean that Note B now has a better match with Note A. This means that Note B should be rendered as well. Therefore, it is possible that notes that are not requested need to have their linked notes regenerated as well.
#[allow(clippy::too_many_lines)]
pub async fn render_notes(
    db: &SqlitePool,
    body: RenderNotesRequest,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<(), Error> {
    let RenderNotesRequest {
        generate_files_note_ids,
        overridden_output_raw_dir,
        include_linked_notes,
        include_cards,
        generate_rendered,
        force_generate_rendered,
    } = body;

    // Assign values for linked notes
    // This must be done at the very end. This is because imagine a note is created referencing a keyword (linked note) but there is no good note with that keyword yet. That note is created later, but the first note will have no linked notes stored in db.
    let notes_data: Vec<RenderNoteData> = sqlx::query_as(
        r"SELECT
              n.id as note_id,
              n.data,
              n.keywords,
              n.custom_data,
              p.name as parser_name,
              GROUP_CONCAT(t.name, ',') AS tags_str
            FROM
              note n
            LEFT JOIN
              note_tag nt ON n.id = nt.note_id
            LEFT JOIN
              tag t ON t.id = nt.tag_id
            LEFT JOIN
              parser p ON n.parser_id = p.id
            WHERE
              t.query IS NULL
            GROUP BY
              n.id
            ORDER BY
              n.id",
    )
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    // let mut linked_notes_map: Option<HashMap<&i64, Vec<(String, Option<(i64, String)>)>>> = None;
    let mut linked_notes_map: Option<HashMap<_, _>> = None;
    if include_linked_notes {
        let notes_data_grouped = notes_data
            .iter()
            .map(|x| (x.parser_name.as_str(), x))
            .into_group_map();
        let note_links = match_keyword_to_linked_note(db, notes_data_grouped, all_parsers).await?;
        // Delete existing note links
        // let _query_result = if let Some(ref note_ids) = note_ids {
        //     // See caveat in `RenderNotesRequest` documentation
        //     let query_str = format!(
        //         "DELETE FROM note_link WHERE parent_note_id IN ({})",
        //         vec!["?"; note_ids.len()].join(", ")
        //     );
        //     let mut query = sqlx::query(query_str.as_str());
        //     for note_id in note_ids {
        //         query = query.bind(note_id);
        //     }
        //     query
        //         .execute(db)
        //         .await
        //         .map_err(|e| SparesError::Sqlx { source: e })?;
        // } else {
        //     sqlx::query("DELETE FROM note_link")
        //         .execute(db)
        //         .await
        //         .map_err(|e| SparesError::Sqlx { source: e })?;
        // };
        let _query_result = sqlx::query("DELETE FROM note_link")
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        // Create note links
        if !note_links.is_empty() {
            let query_str = format!(
                "INSERT INTO note_link (parent_note_id, linked_note_id, \"order\", searched_keyword, matched_keyword) VALUES {}",
                vec!["(?, ?, ?, ?, ?)"; note_links.len()].join(", ")
            );
            let mut query = sqlx::query(query_str.as_str());
            for NoteLink {
                // id: _,
                parent_note_id,
                linked_note_id,
                order,
                searched_keyword,
                matched_keyword,
            } in &note_links
            {
                query = query.bind(parent_note_id);
                query = query.bind(linked_note_id);
                query = query.bind(order);
                query = query.bind(searched_keyword);
                query = query.bind(matched_keyword);
            }
            let _insert_result = query
                .execute(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        }
        linked_notes_map = Some(
            note_links
                .into_iter()
                .map(|note_link| (note_link.parent_note_id, note_link))
                .into_group_map(),
        );
    }

    // Update config
    let mut config = read_internal_config()?;
    config.linked_notes_generated = true;
    write_internal_config(&config)?;

    let note_ids = if let Some(note_ids_search) = generate_files_note_ids {
        match note_ids_search {
            GenerateFilesNoteIds::Query(query) => {
                let evaluator = Evaluator::new(&query);
                Some(
                    evaluator
                        .get_note_ids(db)
                        .await?
                        .into_iter()
                        .collect::<Vec<_>>(),
                )
            }
            GenerateFilesNoteIds::NoteIds(vec) => Some(vec),
        }
    } else {
        None
    };

    // Generate files for notes and cards
    // This must be done after linking because the links need to be shown in the rendered note.
    //
    // Group notes by parser
    let grouped_parse_note_requests = notes_data
        .iter()
        .filter(|render_note_data| {
            note_ids
                .as_ref()
                .is_none_or(|note_ids| note_ids.contains(&render_note_data.note_id))
        })
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
        };
        let _card_paths = create_note_files_bulk(parser.as_ref(), &generate_note_files_requests)?
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
    }
    Ok(())
}

fn render_note_data_to_generate_files_request(
    render_note_data: &RenderNoteData,
    linked_notes_map: Option<&HashMap<NoteId, Vec<NoteLink>>>,
) -> GenerateNoteFilesRequest {
    let RenderNoteData {
        note_id,
        data,
        keywords,
        custom_data,
        parser_name: _,
        tags_str,
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
                        searched_keyword: searched_keyword.to_string(),
                        linked_note_id: *linked_note_id,
                        matched_keyword: matched_keyword.clone(),
                    },
                )
                .collect::<Vec<_>>()
        })
    });
    let keywords = parse_list(keywords);
    let mut tags = parse_list(tags_str);
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
    use crate::{
        api::note::{basic::tests::create_note_helper, render_notes},
        model::NoteLink,
        parsers::get_all_parsers,
        schema::note::RenderNotesRequest,
    };
    use sqlx::SqlitePool;

    #[sqlx::test]
    async fn test_render_note(pool: SqlitePool) -> () {
        let _ = create_note_helper(&pool).await;
        let body = RenderNotesRequest {
            generate_files_note_ids: None,
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
        assert_eq!(note_links.unwrap().len(), 3);
    }
}
