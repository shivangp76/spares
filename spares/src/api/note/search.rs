use super::{MAX_KEYWORD_DIFFERENCE_SCORE, enrich_note};
use crate::{
    Error,
    config::read_internal_config,
    helpers::parse_list,
    model::NoteId,
    schema::{
        card::CardResponse,
        note::{
            MatchedKeywordResponse, SearchKeywordRequest, SearchNotesRequest, SearchNotesResponse,
            UnmatchedKeywordResponse,
        },
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
) -> Result<Vec<MatchedKeywordResponse>, Error> {
    let SearchKeywordRequest {
        keyword: searched_keyword,
    } = body;
    let keywords = get_keywords(db).await?;
    let mut results = keywords
        .into_iter()
        .map(|(id, keyword)| {
            let score = strsim::levenshtein(searched_keyword.as_str(), keyword.as_str());
            (keyword, id, score as u32)
        })
        .filter(|(_, _, score)| *score <= MAX_KEYWORD_DIFFERENCE_SCORE as u32)
        .map(|x| MatchedKeywordResponse {
            matched_keyword: x.0,
            note_id: x.1,
            score: x.2,
        })
        .collect::<Vec<_>>();

    // Sort by score (ascending - lower scores are better matches)
    results.sort_by_key(|x| x.score);

    Ok(results)
}

pub async fn get_unmatched_keywords(
    db: &SqlitePool,
) -> Result<Vec<UnmatchedKeywordResponse>, Error> {
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
