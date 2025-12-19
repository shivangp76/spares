use super::enrich_note;
use crate::{
    Error,
    api::note::match_keyword,
    config::read_internal_config,
    model::{NoteId, NoteLink},
    schema::{
        card::CardResponse,
        note::{
            MatchedKeywordResponse, NoteLinksRequest, SearchKeywordRequest, SearchNotesRequest,
            SearchNotesResponse, UnmatchedKeywordResponse,
        },
    },
    search::evaluator::Evaluator,
};
use sqlx::sqlite::SqlitePool;
use std::collections::HashMap;

pub async fn get_keywords(db: &SqlitePool) -> Result<Vec<(NoteId, String)>, Error> {
    let keywords_data: Vec<(NoteId, String)> =
        sqlx::query_as(r"SELECT note_id, keyword FROM note_keyword")
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;

    Ok(keywords_data)
}

pub async fn search_notes(
    db: &SqlitePool,
    body: SearchNotesRequest,
) -> Result<SearchNotesResponse, Error> {
    let SearchNotesRequest { query, output_type } = body;
    let evaluator = Evaluator::new(&query);
    match output_type {
        crate::search::QueryReturnItemType::Cards => {
            let cards = evaluator.get_cards(db).await?;
            let card_responses = cards
                .into_iter()
                .map(|(card, parser_name)| (CardResponse::new(&card), parser_name))
                .collect::<Vec<_>>();
            Ok(SearchNotesResponse::Cards(card_responses))
        }
        crate::search::QueryReturnItemType::Notes => {
            let notes = evaluator.get_notes(db).await?;
            let mut note_responses = Vec::new();
            let config = read_internal_config()?;
            for (note, parser_name) in notes {
                note_responses.push((
                    enrich_note(db, &note, config.linked_notes_generated).await?,
                    parser_name,
                ));
            }
            Ok(SearchNotesResponse::Notes(note_responses))
        }
    }
}

pub async fn search_keyword(
    db: &SqlitePool,
    body: SearchKeywordRequest,
) -> Result<Vec<MatchedKeywordResponse>, Error> {
    let SearchKeywordRequest {
        keyword: searched_keyword,
    } = body;
    let keywords = get_keywords(db).await?;
    let mut matched_keyword_data = match_keyword(searched_keyword.as_str(), keywords.as_ref());

    // Sort by score (ascending - lower scores are better matches)
    matched_keyword_data.sort_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            // NaNs to sort last (so valid scores come first)
            .unwrap_or(std::cmp::Ordering::Greater)
    });

    Ok(matched_keyword_data)
}

pub async fn get_unmatched_keywords(
    db: &SqlitePool,
) -> Result<Vec<UnmatchedKeywordResponse>, Error> {
    // TODO: This might be because linked notes were not generated, so either this needs to be documented or the function needs to return an error if the linked notes are not generated.
    let unmatched_keywords: Vec<(NoteId, String)> = sqlx::query_as(
        r"SELECT parent_note_id, searched_keyword
         FROM note_link
         WHERE linked_note_id IS NULL AND matched_keyword IS NULL",
    )
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    Ok(unmatched_keywords
        .into_iter()
        .map(|(note_id, searched_keyword)| UnmatchedKeywordResponse {
            note_id,
            searched_keyword,
        })
        .collect())
}

pub async fn get_note_links(
    db: &SqlitePool,
    body: NoteLinksRequest,
) -> Result<Vec<NoteLink>, Error> {
    let NoteLinksRequest { score_threshold } = body;

    let mut note_links: Vec<NoteLink> = sqlx::query_as(
        r"SELECT * FROM note_link
         WHERE linked_note_id IS NOT NULL AND matched_keyword IS NOT NULL
         ORDER BY score ASC",
    )
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    // binary_search_by returns Ok(idx) if an exact match was found,
    // otherwise Err(idx) = insertion point.
    let cut = match note_links.binary_search_by(|nl| match nl.score {
        Some(s) => s
            .partial_cmp(&score_threshold)
            .unwrap_or(std::cmp::Ordering::Greater), // NaN = +∞
        None => std::cmp::Ordering::Less, // NULL = -∞
    }) {
        Ok(idx) => idx + 1, // include the matching element
        Err(idx) => idx,    // idx is the first element > threshold
    };
    note_links.truncate(cut);

    Ok(note_links)
}

pub async fn get_duplicate_keywords(db: &SqlitePool) -> Result<Vec<(String, Vec<NoteId>)>, Error> {
    let keywords = get_keywords(db).await?;
    let mut keyword_map: HashMap<String, Vec<NoteId>> = HashMap::new();
    for (note_id, keyword) in keywords {
        keyword_map.entry(keyword).or_default().push(note_id);
    }
    let duplicates: Vec<(String, Vec<NoteId>)> = keyword_map
        .into_iter()
        .filter(|(_, note_ids)| note_ids.len() > 1)
        .collect();
    Ok(duplicates)
}
