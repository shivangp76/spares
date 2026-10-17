use std::collections::HashMap;

use sqlx::sqlite::SqlitePool;

use super::enrich_note;
use crate::Error;
use crate::api::note::match_keyword;
use crate::config::read_internal_config;
use crate::model::Card;
use crate::model::Note;
use crate::model::NoteId;
use crate::model::NoteLink;
use crate::schema::card::CardResponse;
use crate::schema::note::MatchedKeywordResponse;
use crate::schema::note::NoteLinksRequest;
use crate::schema::note::NoteMatch;
use crate::schema::note::SearchKeywordRequest;
use crate::schema::note::SearchNotesRequest;
use crate::schema::note::SearchNotesResponse;
use crate::schema::note::SearchPageItems;
use crate::schema::note::SearchPageRequest;
use crate::schema::note::SearchPageResponse;
use crate::schema::note::UnmatchedKeywordResponse;
use crate::search::QueryReturnItemType;
use crate::search::evaluator::Evaluator;
use crate::search::evaluator::text_matches;
use crate::search::evaluator::text_patterns;

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
    let evaluator = Evaluator::new(&query).ranked();
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
            let config = read_internal_config(db).await?;
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

/// The most results a page of [`search_page`] has.
const MAX_PAGE_LIMIT: usize = 1000;

/// One page of the results of a search, with how many there are in all, and for notes, where
/// each note's text matches.
pub async fn search_page(
    db: &SqlitePool,
    body: SearchPageRequest,
) -> Result<SearchPageResponse, Error> {
    let SearchPageRequest {
        query,
        output_type,
        page,
        limit,
    } = body;
    let limit = limit.clamp(1, MAX_PAGE_LIMIT);
    let offset = page.max(1).saturating_sub(1).saturating_mul(limit);
    // Only the ids of every result are read, and then the page's items in full.
    let in_list = |ids: &mut dyn Iterator<Item = i64>| {
        ids.map(|id| id.to_string()).collect::<Vec<_>>().join(", ")
    };
    match output_type {
        QueryReturnItemType::Notes => {
            #[derive(sqlx::FromRow)]
            struct NoteWithParser {
                #[sqlx(flatten)]
                note: Note,
                name: String,
            }
            let ids = Evaluator::new(&query).get_note_ids(db).await?;
            let page_ids = ids.iter().skip(offset).take(limit).collect::<Vec<_>>();
            let rows: Vec<NoteWithParser> = sqlx::query_as(&format!(
                "SELECT n.*, p.name FROM note n LEFT JOIN parser p ON n.parser_id = p.id WHERE n.id IN ({})",
                in_list(&mut page_ids.iter().map(|id| id.packed()))
            ))
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
            let mut rows = rows
                .into_iter()
                .map(|row| (row.note.id, row))
                .collect::<HashMap<_, _>>();
            let patterns = text_patterns(&query);
            let config = read_internal_config(db).await?;
            let mut notes = Vec::with_capacity(page_ids.len());
            for id in page_ids {
                // A note deleted since its id was read is left out.
                let Some(row) = rows.remove(id) else { continue };
                notes.push(NoteMatch {
                    matches: text_matches(&patterns, &row.note.data),
                    note: enrich_note(db, &row.note, config.linked_notes_generated).await?,
                    parser_name: row.name,
                });
            }
            Ok(SearchPageResponse {
                total: ids.len(),
                items: SearchPageItems::Notes(notes),
            })
        }
        QueryReturnItemType::Cards => {
            #[derive(sqlx::FromRow)]
            struct CardWithParser {
                #[sqlx(flatten)]
                card: Card,
                name: String,
            }
            let ids = Evaluator::new(&query).ranked().get_card_ids(db).await?;
            let page_ids = ids.iter().skip(offset).take(limit).collect::<Vec<_>>();
            let rows: Vec<CardWithParser> = sqlx::query_as(&format!(
                "SELECT c.*, p.name FROM card c LEFT JOIN note n ON n.id = c.note_id LEFT JOIN parser p ON n.parser_id = p.id WHERE c.id IN ({})",
                in_list(&mut page_ids.iter().map(|id| **id))
            ))
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
            let mut rows = rows
                .into_iter()
                .map(|row| (row.card.id, row))
                .collect::<HashMap<_, _>>();
            let cards = page_ids
                .into_iter()
                .filter_map(|id| rows.remove(id))
                .map(|row| (CardResponse::new(&row.card), row.name))
                .collect();
            Ok(SearchPageResponse {
                total: ids.len(),
                items: SearchPageItems::Cards(cards),
            })
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
    let config = read_internal_config(db).await?;
    if !config.linked_notes_generated {
        return Err(crate::LibraryError::InvalidConfig(
            "Linked notes have not been generated yet. Run the link generation step first."
                .to_string(),
        )
        .into());
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn pages_through_results(
        pool_options: sqlx::sqlite::SqlitePoolOptions,
        options: sqlx::sqlite::SqliteConnectOptions,
    ) {
        let pool = crate::helpers::pool_with_regexp(pool_options, options).await;
        sqlx::query("INSERT INTO parser (id, name) VALUES (1, 'markdown')")
            .execute(&pool)
            .await
            .unwrap();
        for (i, letters) in ["a", "b", "c", "d", "e"].into_iter().enumerate() {
            let note_id = letters.parse::<NoteId>().unwrap();
            sqlx::query(
                "INSERT INTO note (id, data, created_at, updated_at, custom_data, parser_id) VALUES (?, ?, 0, 0, '{}', 1)",
            )
            .bind(note_id)
            .bind(format!("dog {i} and a cat"))
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                r#"INSERT INTO card (note_id, "order", back_type, due, stability, difficulty,
                   desired_retention, state, custom_data) VALUES (?, 1, 1, ?, 0, 0, 0.9, 0, '{}')"#,
            )
            .bind(note_id)
            .bind(i64::try_from(10 - i).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        }
        let page = async |output_type, page, limit| {
            search_page(
                &pool,
                SearchPageRequest {
                    query: "dog -e sort_by_desc=id".to_string(),
                    output_type,
                    page,
                    limit,
                },
            )
            .await
            .unwrap()
        };

        let notes = page(QueryReturnItemType::Notes, 2, 2).await;
        assert_eq!(notes.total, 5);
        let SearchPageItems::Notes(notes) = notes.items else {
            panic!("asked for notes")
        };
        assert_eq!(
            notes
                .iter()
                .map(|n| (n.note.id.to_string(), n.matches.clone()))
                .collect::<Vec<_>>(),
            [
                ("c".to_string(), vec![(0, 3)]),
                ("b".to_string(), vec![(0, 3)])
            ]
        );
        assert_eq!(notes[0].parser_name, "markdown");

        let past_the_end = page(QueryReturnItemType::Notes, 4, 2).await;
        assert!(matches!(past_the_end.items, SearchPageItems::Notes(n) if n.is_empty()));

        let cards = page(QueryReturnItemType::Cards, 1, 3).await;
        let SearchPageItems::Cards(cards) = cards.items else {
            panic!("asked for cards")
        };
        assert_eq!(
            cards
                .iter()
                .map(|(card, _)| card.note_id.to_string())
                .collect::<Vec<_>>(),
            ["e", "d", "c"]
        );
    }
}
