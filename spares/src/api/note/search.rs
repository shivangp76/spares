use super::{MAX_KEYWORD_DIFFERENCE_SCORE, enrich_note};
use crate::{
    Error,
    config::read_internal_config,
    helpers::parse_list,
    model::NoteId,
    schema::{
        card::CardResponse,
        note::{SearchKeywordRequest, SearchNotesRequest, SearchNotesResponse},
    },
    search::evaluator::Evaluator,
};
use sqlx::sqlite::SqlitePool;

pub async fn get_keywords(db: &SqlitePool) -> Result<Vec<(NoteId, String)>, Error> {
    let keywords_data: Vec<(NoteId, String)> = sqlx::query_as(r"SELECT id, keywords FROM note")
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    Ok(keywords_data
        .into_iter()
        .flat_map(|(id, keywords)| {
            parse_list(keywords.as_str())
                .into_iter()
                .map(|k| (id, k))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>())
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
) -> Result<Option<(NoteId, String)>, Error> {
    let SearchKeywordRequest {
        keyword: searched_keyword,
    } = body;
    let keywords = get_keywords(db).await?;
    let closest_keyword = keywords.into_iter().min_by_key(|(_id, keyword)| {
        // Do not return matches that are too far apart
        Some(strsim::levenshtein(searched_keyword.as_str(), keyword))
            .filter(|&score| score <= MAX_KEYWORD_DIFFERENCE_SCORE)
    });
    if let Some((result_note_id, result_keyword)) = closest_keyword {
        // let result_note = get_note(db, result_note_id).await?;
        return Ok(Some((result_note_id, result_keyword.to_string())));
    }
    Ok(None)
}
