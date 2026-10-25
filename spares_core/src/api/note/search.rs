use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::Entry;

use chrono::DateTime;
use sqlx::sqlite::SqlitePool;

use super::enrich_note;
use crate::Error;
use crate::LibraryError;
use crate::api::note::get_note_links_map;
use crate::api::note::match_keyword;
use crate::api::placeholders;
use crate::config::read_external_config;
use crate::config::read_internal_config;
use crate::model::Card;
use crate::model::CardId;
use crate::model::Note;
use crate::model::NoteId;
use crate::model::NoteLink;
use crate::model::Score;
use crate::model::SpecialState;
use crate::schema::card::CardResponse;
use crate::schema::note::CardMatch;
use crate::schema::note::CardStats;
use crate::schema::note::GraphEdge;
use crate::schema::note::GraphNode;
use crate::schema::note::GraphNodeCards;
use crate::schema::note::GraphRelation;
use crate::schema::note::MAX_GRAPH_NODES;
use crate::schema::note::MatchedKeywordResponse;
use crate::schema::note::NoteGraphRequest;
use crate::schema::note::NoteGraphResponse;
use crate::schema::note::NoteLinksRequest;
use crate::schema::note::NoteMatch;
use crate::schema::note::SearchKeywordRequest;
use crate::schema::note::SearchNotesRequest;
use crate::schema::note::SearchNotesResponse;
use crate::schema::note::SearchPageItems;
use crate::schema::note::SearchPageRequest;
use crate::schema::note::SearchPageResponse;
use crate::schema::note::SearchSort;
use crate::schema::note::UnmatchedKeywordResponse;
use crate::schema::note::UnmatchedLink;
use crate::search::QueryReturnItemType;
use crate::search::evaluator::Evaluator;
use crate::search::evaluator::interval_sql;
use crate::search::evaluator::lapses_sql;
use crate::search::evaluator::last_rated_sql;
use crate::search::evaluator::reps_sql;
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
        sort,
    } = body;
    let sorted_query = match sort {
        Some(sort) => sorted_query(&query, &sort)?,
        None => query.clone(),
    };
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
            let ids = match Evaluator::new(&sorted_query).get_note_ids(db).await {
                Ok(ids) => ids,
                Err(e) => return Err(query_error(db, &query, &sorted_query, e).await),
            };
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
            let ids = match Evaluator::new(&sorted_query)
                .ranked()
                .get_card_ids(db)
                .await
            {
                Ok(ids) => ids,
                Err(e) => return Err(query_error(db, &query, &sorted_query, e).await),
            };
            let page_ids = ids.iter().skip(offset).take(limit).collect::<Vec<_>>();
            let mut rows = card_matches(db, &in_list(&mut page_ids.iter().map(|id| **id))).await?;
            let cards = page_ids
                .into_iter()
                .filter_map(|id| rows.remove(id))
                .collect();
            Ok(SearchPageResponse {
                total: ids.len(),
                items: SearchPageItems::Cards(cards),
            })
        }
    }
}

/// The cards `ids_sql`, a list of ids for `IN`, with what their reviews say, by id.
async fn card_matches(db: &SqlitePool, ids_sql: &str) -> Result<HashMap<CardId, CardMatch>, Error> {
    #[derive(sqlx::FromRow)]
    struct CardWithParser {
        #[sqlx(flatten)]
        card: Card,
        name: String,
        interval_days: Option<f64>,
        reps: u32,
        lapses: u32,
        last_reviewed: Option<i64>,
    }
    let rows: Vec<CardWithParser> = sqlx::query_as(&format!(
        "SELECT c.*, p.name, {} AS interval_days, {} AS reps, {} AS lapses, {} AS last_reviewed
        FROM card c LEFT JOIN note n ON n.id = c.note_id LEFT JOIN parser p ON n.parser_id = p.id
        WHERE c.id IN ({ids_sql})",
        interval_sql(),
        reps_sql(),
        lapses_sql(),
        last_rated_sql(),
    ))
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let card_match = CardMatch {
                card: CardResponse::new(&row.card),
                parser_name: row.name,
                stats: CardStats {
                    interval_days: row.interval_days,
                    reps: row.reps,
                    lapses: row.lapses,
                    last_reviewed: row
                        .last_reviewed
                        .and_then(|at| DateTime::from_timestamp(at, 0)),
                },
            };
            (row.card.id, card_match)
        })
        .collect())
}

/// The error of `query` alone, whose place in the query is right, as it isn't in the query with the
/// sort added, or `error` if `query` alone has none.
async fn query_error(db: &SqlitePool, query: &str, sorted_query: &str, error: Error) -> Error {
    if query == sorted_query {
        return error;
    }
    match Evaluator::new(query).get_note_ids(db).await {
        Ok(_) => error,
        Err(e) => e,
    }
}

/// `query`, sorted by `sort` first.
fn sorted_query(query: &str, sort: &SearchSort) -> Result<String, Error> {
    // The field goes into the query, so it must be only a field.
    if sort.field.is_empty()
        || !sort
            .field
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return Err(Error::Library(LibraryError::InvalidConfig(format!(
            "Can't sort by `{}`, which is not a field",
            sort.field
        ))));
    }
    let direction = if sort.descending { "desc" } else { "asc" };
    let term = format!("sort_by_{direction}={}", sort.field);
    Ok(if query.trim().is_empty() {
        term
    } else {
        // Groups are evaluated in order, before the terms beside them, so the sort is in one of
        // its own to come first. The query's keeps a top-level `or` in it from taking the sort.
        format!("({term}) ({query})")
    })
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

/// The notes a query finds and the notes they build on. See [`NoteGraphRequest`].
pub async fn get_note_graph(
    db: &SqlitePool,
    body: NoteGraphRequest,
) -> Result<NoteGraphResponse, Error> {
    let NoteGraphRequest {
        query,
        max_score,
        ancestor_depth,
        descendant_depth,
        groups,
        max_nodes,
    } = body;
    let max_score = match max_score {
        Some(score) => score,
        None => read_external_config()?.queue.linked_note_max_score,
    };
    let found: Vec<NoteId> = if query.trim().is_empty() {
        sqlx::query_scalar(r"SELECT id FROM note ORDER BY id")
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?
    } else {
        Evaluator::new(&query).get_note_ids(db).await?
    };
    let mut walk = GraphWalk {
        max_score,
        max_nodes: max_nodes.clamp(1, MAX_GRAPH_NODES),
        truncated: false,
        reached: HashMap::new(),
        order: Vec::new(),
    };
    for id in found {
        walk.reach(id, GraphRelation::Found, 0);
    }
    let found = walk.order.clone();
    walk.follow(db, &found, GraphRelation::Prerequisite, ancestor_depth)
        .await?;
    walk.follow(db, &found, GraphRelation::Dependent, descendant_depth)
        .await?;

    // Every link between two notes shown, including those not followed to reach them
    let links_map = get_note_links_map(db, &walk.order).await?;
    let mut seen_edges = HashSet::new();
    let mut edges = Vec::new();
    let mut unmatched = Vec::new();
    for link in walk
        .order
        .iter()
        .filter_map(|id| links_map.get(id))
        .flatten()
    {
        match link.linked_note_id {
            None => unmatched.push(UnmatchedLink {
                from: link.parent_note_id,
                keyword: link.searched_keyword.clone(),
            }),
            Some(to)
                if walk.reached.contains_key(&to)
                    && walk.keeps(link)
                    && seen_edges.insert((link.parent_note_id, to)) =>
            {
                edges.push(GraphEdge {
                    from: link.parent_note_id,
                    to,
                    keyword: link
                        .matched_keyword
                        .clone()
                        .unwrap_or_else(|| link.searched_keyword.clone()),
                    score: link.score,
                });
            }
            Some(_) => {}
        }
    }

    let mut group_of: HashMap<NoteId, usize> = HashMap::new();
    for (i, group_query) in groups.iter().enumerate() {
        if group_query.trim().is_empty() {
            continue;
        }
        for id in Evaluator::new(group_query).get_note_ids(db).await? {
            if walk.reached.contains_key(&id) {
                group_of.entry(id).or_insert(i);
            }
        }
    }

    Ok(NoteGraphResponse {
        nodes: graph_nodes(db, &walk, &group_of).await?,
        edges,
        unmatched,
        truncated: walk.truncated,
        links_generated: read_internal_config(db).await?.linked_notes_generated,
    })
}

/// The notes a graph has reached so far, in the order they were reached.
struct GraphWalk {
    max_score: Score,
    max_nodes: usize,
    truncated: bool,
    reached: HashMap<NoteId, (GraphRelation, u32)>,
    order: Vec<NoteId>,
}

impl GraphWalk {
    /// Whether `link` is close enough a match to follow.
    fn keeps(&self, link: &NoteLink) -> bool {
        link.score.is_none_or(|s| s <= self.max_score)
    }

    /// Adds `id` unless it is already there, returning whether it was added.
    fn reach(&mut self, id: NoteId, relation: GraphRelation, depth: u32) -> bool {
        let Entry::Vacant(entry) = self.reached.entry(id) else {
            return false;
        };
        if self.order.len() >= self.max_nodes {
            self.truncated = true;
            return false;
        }
        entry.insert((relation, depth));
        self.order.push(id);
        true
    }

    /// Follows links from `start` up to the notes they build on, for `Prerequisite`, or down to
    /// the notes that build on them, for `Dependent`, up to `max_depth` links away. Breadth
    /// first, so that when notes are left out, they are the furthest away.
    async fn follow(
        &mut self,
        db: &SqlitePool,
        start: &[NoteId],
        relation: GraphRelation,
        max_depth: Option<u32>,
    ) -> Result<(), Error> {
        let mut level = start.to_vec();
        let mut depth = 0;
        while !level.is_empty() && max_depth.is_none_or(|max| depth < max) {
            depth += 1;
            let mut next = Vec::new();
            let neighbors: Vec<NoteId> = if relation == GraphRelation::Dependent {
                links_to(db, &level)
                    .await?
                    .into_iter()
                    .filter(|link| self.keeps(link))
                    .map(|link| link.parent_note_id)
                    .collect()
            } else {
                let links_map = get_note_links_map(db, &level).await?;
                level
                    .iter()
                    .filter_map(|id| links_map.get(id))
                    .flatten()
                    .filter(|link| self.keeps(link))
                    .filter_map(|link| link.linked_note_id)
                    .collect()
            };
            for id in neighbors {
                if self.reach(id, relation, depth) {
                    next.push(id);
                }
            }
            level = next;
        }
        Ok(())
    }
}

/// The links to any of `note_ids`.
async fn links_to(db: &SqlitePool, note_ids: &[NoteId]) -> Result<Vec<NoteLink>, Error> {
    let mut links = Vec::new();
    // Under SQLite's limit on the number of bound values
    for chunk in note_ids.chunks(MAX_GRAPH_NODES) {
        let sql = format!(
            "SELECT * FROM note_link WHERE linked_note_id IN ({}) ORDER BY parent_note_id, \"order\"",
            placeholders(chunk.len())
        );
        let mut query = sqlx::query_as::<_, NoteLink>(&sql);
        for id in chunk {
            query = query.bind(id);
        }
        links.extend(
            query
                .fetch_all(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?,
        );
    }
    Ok(links)
}

/// The keywords, tags and cards of the notes `walk` reached, in the order it reached them.
async fn graph_nodes(
    db: &SqlitePool,
    walk: &GraphWalk,
    group_of: &HashMap<NoteId, usize>,
) -> Result<Vec<GraphNode>, Error> {
    let order = &walk.order;
    let mut keywords: HashMap<NoteId, Vec<String>> = HashMap::new();
    let mut tags: HashMap<NoteId, Vec<String>> = HashMap::new();
    let mut cards: HashMap<NoteId, GraphNodeCards> = HashMap::new();
    // Under SQLite's limit on the number of bound values
    for chunk in order.chunks(MAX_GRAPH_NODES) {
        let in_list = placeholders(chunk.len());
        let keyword_sql = format!(
            "SELECT note_id, keyword FROM note_keyword WHERE note_id IN ({in_list}) ORDER BY rowid"
        );
        let tag_sql = format!(
            "SELECT nt.note_id, t.name FROM note_tag nt JOIN tag t ON t.id = nt.tag_id \
             WHERE nt.note_id IN ({in_list}) ORDER BY t.name"
        );
        let card_sql = format!(
            "SELECT note_id, state, special_state = {suspended} FROM card WHERE note_id IN ({in_list})",
            suspended = SpecialState::Suspended as u8,
        );
        let mut keyword_query = sqlx::query_as::<_, (NoteId, String)>(&keyword_sql);
        let mut tag_query = sqlx::query_as::<_, (NoteId, String)>(&tag_sql);
        let mut card_query = sqlx::query_as::<_, (NoteId, u32, Option<bool>)>(&card_sql);
        for id in chunk {
            keyword_query = keyword_query.bind(id);
            tag_query = tag_query.bind(id);
            card_query = card_query.bind(id);
        }
        for (id, keyword) in keyword_query
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?
        {
            keywords.entry(id).or_default().push(keyword);
        }
        for (id, tag) in tag_query
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?
        {
            tags.entry(id).or_default().push(tag);
        }
        for (id, state, suspended) in card_query
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?
        {
            let counts = cards.entry(id).or_default();
            match (suspended.unwrap_or(false), state) {
                (true, _) => counts.suspended += 1,
                (false, 0) => counts.new += 1,
                (false, 2) => counts.review += 1,
                (false, _) => counts.learning += 1,
            }
        }
    }
    Ok(order
        .iter()
        .map(|&id| {
            let (relation, depth) = walk.reached[&id];
            GraphNode {
                id,
                keywords: keywords.remove(&id).unwrap_or_default(),
                tags: tags.remove(&id).unwrap_or_default(),
                relation,
                depth,
                group: group_of.get(&id).copied(),
                cards: cards.remove(&id).unwrap_or_default(),
            }
        })
        .collect())
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
                    sort: None,
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
                .map(|m| m.card.note_id.to_string())
                .collect::<Vec<_>>(),
            ["e", "d", "c"]
        );
    }

    /// Notes `a` to `c`, each with a card, where `a` is due last.
    async fn sort_pool(
        pool_options: sqlx::sqlite::SqlitePoolOptions,
        options: sqlx::sqlite::SqliteConnectOptions,
    ) -> SqlitePool {
        let pool = crate::helpers::pool_with_regexp(pool_options, options).await;
        sqlx::query("INSERT INTO parser (id, name) VALUES (1, 'markdown')")
            .execute(&pool)
            .await
            .unwrap();
        for (letters, due) in [("a", 300_000), ("b", 100_000), ("c", 200_000)] {
            let note_id = letters.parse::<NoteId>().unwrap();
            sqlx::query(
                "INSERT INTO note (id, data, created_at, updated_at, custom_data, parser_id) VALUES (?, ?, 0, 0, '{}', 1)",
            )
            .bind(note_id)
            .bind(format!("dog {letters}"))
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                r#"INSERT INTO card (note_id, "order", back_type, due, stability, difficulty,
                   desired_retention, state, custom_data) VALUES (?, 1, 1, ?, 0, 0, 0.9, 2, '{}')"#,
            )
            .bind(note_id)
            .bind(due)
            .execute(&pool)
            .await
            .unwrap();
        }
        pool
    }

    async fn card_page(pool: &SqlitePool, query: &str, sort: Option<SearchSort>) -> Vec<CardMatch> {
        let page = search_page(
            pool,
            SearchPageRequest {
                query: query.to_string(),
                output_type: QueryReturnItemType::Cards,
                page: 1,
                limit: 10,
                sort,
            },
        )
        .await
        .unwrap();
        let SearchPageItems::Cards(cards) = page.items else {
            panic!("asked for cards")
        };
        cards
    }

    fn note_ids(cards: &[CardMatch]) -> Vec<String> {
        cards.iter().map(|m| m.card.note_id.to_string()).collect()
    }

    #[sqlx::test]
    async fn sorts_by_a_column_before_the_query(
        pool_options: sqlx::sqlite::SqlitePoolOptions,
        options: sqlx::sqlite::SqliteConnectOptions,
    ) {
        let pool = sort_pool(pool_options, options).await;
        let due = |descending| {
            Some(SearchSort {
                field: "c.due".to_string(),
                descending,
            })
        };
        assert_eq!(
            note_ids(&card_page(&pool, "", due(false)).await),
            ["b", "c", "a"]
        );
        assert_eq!(
            note_ids(&card_page(&pool, "  ", due(true)).await),
            ["a", "c", "b"]
        );
        // The column comes first, and the query's sort breaks ties.
        assert_eq!(
            note_ids(&card_page(&pool, "dog sort_by_asc=id", due(true)).await),
            ["a", "c", "b"]
        );
        // A top-level `or` is kept together.
        assert_eq!(
            note_ids(&card_page(&pool, r#""dog b" or "dog a""#, due(false)).await),
            ["b", "a"]
        );
        // An error in the query is placed in the query as written.
        let bad = search_page(
            &pool,
            SearchPageRequest {
                query: "dog c.nope=1".to_string(),
                output_type: QueryReturnItemType::Cards,
                page: 1,
                limit: 10,
                sort: due(false),
            },
        )
        .await
        .unwrap_err();
        let unsorted = Evaluator::new("dog c.nope=1")
            .get_note_ids(&pool)
            .await
            .unwrap_err();
        assert_eq!(format!("{bad:?}"), format!("{unsorted:?}"));
        let not_a_field = SearchSort {
            field: "c.due tag=x".to_string(),
            descending: false,
        };
        assert!(
            search_page(
                &pool,
                SearchPageRequest {
                    query: String::new(),
                    output_type: QueryReturnItemType::Cards,
                    page: 1,
                    limit: 10,
                    sort: Some(not_a_field),
                },
            )
            .await
            .is_err()
        );
    }

    #[sqlx::test]
    async fn cards_come_with_their_review_stats(
        pool_options: sqlx::sqlite::SqlitePoolOptions,
        options: sqlx::sqlite::SqliteConnectOptions,
    ) {
        let pool = sort_pool(pool_options, options).await;
        let card_id: i64 = sqlx::query_scalar("SELECT id FROM card WHERE due = 300000")
            .fetch_one(&pool)
            .await
            .unwrap();
        // A pass, then a lapse a day before the card is due, and a cram, which is left out.
        sqlx::query(
            r"INSERT INTO review_log (card_id, reviewed_at, kind, rating, scheduler_name,
               scheduled_time, recall_duration, rate_duration, previous_state, custom_data)
               VALUES (?1, 10, 0, 3, 'fsrs', 0, 1, 1, 2, '{}'),
                      (?1, 300000 - 86400, 0, 1, 'fsrs', 0, 1, 1, 2, '{}'),
                      (?1, 250000, 16, 3, 'fsrs', 0, 1, 1, 2, '{}')",
        )
        .bind(card_id)
        .execute(&pool)
        .await
        .unwrap();
        let cards = card_page(&pool, "", None).await;
        let reviewed = cards.iter().find(|m| m.card.id == card_id).unwrap();
        assert_eq!(
            reviewed.stats,
            CardStats {
                interval_days: Some(1.0),
                reps: 2,
                lapses: 1,
                last_reviewed: DateTime::from_timestamp(300_000 - 86400, 0),
            }
        );
        let other = cards.iter().find(|m| m.card.id != card_id).unwrap();
        assert_eq!(other.stats, CardStats::default());
        assert_eq!(reviewed.parser_name, "markdown");
    }

    /// Notes `a` to `e`, where `d` links to `c`, `c` to `b`, `b` to `a` and `e` to `a`, and `a`
    /// back to `d` with a weak link, making a cycle.
    async fn graph_pool(
        pool_options: sqlx::sqlite::SqlitePoolOptions,
        options: sqlx::sqlite::SqliteConnectOptions,
    ) -> SqlitePool {
        let pool = crate::helpers::pool_with_regexp(pool_options, options).await;
        sqlx::query("INSERT INTO parser (id, name) VALUES (1, 'markdown')")
            .execute(&pool)
            .await
            .unwrap();
        for letters in ["a", "b", "c", "d", "e"] {
            let note_id = letters.parse::<NoteId>().unwrap();
            sqlx::query(
                "INSERT INTO note (id, data, created_at, updated_at, custom_data, parser_id) VALUES (?, ?, 0, 0, '{}', 1)",
            )
            .bind(note_id)
            .bind(format!("note {letters}"))
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO note_keyword (note_id, keyword, embedded) VALUES (?, ?, 0)")
                .bind(note_id)
                .bind(format!("{letters}-word"))
                .execute(&pool)
                .await
                .unwrap();
        }
        for (from, to, score) in [
            ("d", "c", 0.0),
            ("c", "b", 0.0),
            ("b", "a", 1.0),
            ("e", "a", 0.0),
            ("a", "d", 3.0),
        ] {
            sqlx::query(
                r#"INSERT INTO note_link (parent_note_id, linked_note_id, "order", searched_keyword, matched_keyword, score)
                   VALUES (?, ?, 0, ?, ?, ?)"#,
            )
            .bind(from.parse::<NoteId>().unwrap())
            .bind(to.parse::<NoteId>().unwrap())
            .bind(format!("{to}-word"))
            .bind(format!("{to}-word"))
            .bind(score)
            .execute(&pool)
            .await
            .unwrap();
        }
        // `e` also links to a keyword no note has
        sqlx::query(
            r#"INSERT INTO note_link (parent_note_id, linked_note_id, "order", searched_keyword)
               VALUES (?, NULL, 1, 'zeta')"#,
        )
        .bind("e".parse::<NoteId>().unwrap())
        .execute(&pool)
        .await
        .unwrap();
        // A suspended review card and a new one for `c`
        sqlx::query(
            r#"INSERT INTO card (note_id, "order", back_type, due, stability, difficulty,
               desired_retention, state, special_state, custom_data)
               VALUES (?, 1, 1, 0, 0, 0, 0.9, 2, 1, '{}'), (?, 2, 1, 0, 0, 0, 0.9, 0, NULL, '{}')"#,
        )
        .bind("c".parse::<NoteId>().unwrap())
        .bind("c".parse::<NoteId>().unwrap())
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn graph(
        pool: &SqlitePool,
        request: NoteGraphRequest,
    ) -> (Vec<(String, u32)>, Vec<(String, String)>, bool) {
        let graph = get_note_graph(pool, request).await.unwrap();
        (
            graph
                .nodes
                .iter()
                .map(|n| (n.id.letters(), n.depth))
                .collect(),
            graph
                .edges
                .iter()
                .map(|e| (e.from.letters(), e.to.letters()))
                .collect(),
            graph.truncated,
        )
    }

    fn graph_request(query: &str) -> NoteGraphRequest {
        NoteGraphRequest {
            query: query.to_string(),
            max_score: Some(5.0),
            ancestor_depth: None,
            descendant_depth: Some(0),
            groups: Vec::new(),
            max_nodes: 100,
        }
    }

    fn pairs<const N: usize>(items: [(&str, &str); N]) -> Vec<(String, String)> {
        items
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[sqlx::test]
    async fn graph_shows_ancestors_but_not_descendants_outside_the_query(
        pool_options: sqlx::sqlite::SqlitePoolOptions,
        options: sqlx::sqlite::SqliteConnectOptions,
    ) {
        let pool = graph_pool(pool_options, options).await;
        let (nodes, edges, truncated) = graph(&pool, graph_request("id=c")).await;
        // `d` and `e` link to notes shown, but the query doesn't find them. The cycle back
        // through `a` to `d` ends there.
        assert_eq!(
            nodes,
            [
                ("c".to_string(), 0),
                ("b".to_string(), 1),
                ("a".to_string(), 2),
                ("d".to_string(), 3)
            ]
        );
        assert_eq!(
            edges,
            pairs([("c", "b"), ("b", "a"), ("a", "d"), ("d", "c")])
        );
        assert!(!truncated);

        let full = get_note_graph(&pool, graph_request("id=c")).await.unwrap();
        assert_eq!(full.nodes[0].relation, GraphRelation::Found);
        assert_eq!(full.nodes[1].relation, GraphRelation::Prerequisite);
        assert_eq!(full.nodes[0].keywords, ["c-word"]);
        assert_eq!(
            full.nodes[0].cards,
            GraphNodeCards {
                new: 1,
                suspended: 1,
                ..GraphNodeCards::default()
            }
        );
    }

    #[sqlx::test]
    async fn graph_limits_depth_score_and_size(
        pool_options: sqlx::sqlite::SqlitePoolOptions,
        options: sqlx::sqlite::SqliteConnectOptions,
    ) {
        let pool = graph_pool(pool_options, options).await;
        let shallow = NoteGraphRequest {
            ancestor_depth: Some(1),
            ..graph_request("id=d")
        };
        let (nodes, edges, _) = graph(&pool, shallow).await;
        assert_eq!(nodes, [("d".to_string(), 0), ("c".to_string(), 1)]);
        assert_eq!(edges, pairs([("d", "c")]));

        // The links to `a` and back to `d` score too high to follow
        let strict = NoteGraphRequest {
            max_score: Some(0.5),
            ..graph_request("id=d")
        };
        let (nodes, _, _) = graph(&pool, strict).await;
        assert_eq!(
            nodes,
            [
                ("d".to_string(), 0),
                ("c".to_string(), 1),
                ("b".to_string(), 2)
            ]
        );

        let small = NoteGraphRequest {
            max_nodes: 2,
            ..graph_request("id=d")
        };
        let (nodes, edges, truncated) = graph(&pool, small).await;
        assert_eq!(nodes.len(), 2);
        assert_eq!(edges, pairs([("d", "c")]));
        assert!(truncated);

        // An empty query finds every note
        let (nodes, edges, _) = graph(&pool, graph_request(" ")).await;
        assert_eq!(nodes.len(), 5);
        assert_eq!(edges.len(), 5);
    }

    #[sqlx::test]
    async fn graph_follows_dependents_groups_and_unmatched_links(
        pool_options: sqlx::sqlite::SqlitePoolOptions,
        options: sqlx::sqlite::SqliteConnectOptions,
    ) {
        let pool = graph_pool(pool_options, options).await;
        // Down from `a`, to `b` and `e` that link to it, then `c`; and up, through the weak link,
        // to `d`, which also links to `c`
        let request = NoteGraphRequest {
            descendant_depth: Some(2),
            ancestor_depth: Some(1),
            groups: vec![
                String::new(),
                "id=b or id=c".to_string(),
                "id=c".to_string(),
            ],
            ..graph_request("id=a")
        };
        let full = get_note_graph(&pool, request).await.unwrap();
        let nodes: Vec<_> = full
            .nodes
            .iter()
            .map(|n| (n.id.letters(), n.relation, n.depth, n.group))
            .collect();
        assert_eq!(
            nodes,
            [
                ("a".to_string(), GraphRelation::Found, 0, None),
                ("d".to_string(), GraphRelation::Prerequisite, 1, None),
                ("b".to_string(), GraphRelation::Dependent, 1, Some(1)),
                ("e".to_string(), GraphRelation::Dependent, 1, None),
                ("c".to_string(), GraphRelation::Dependent, 2, Some(1)),
            ]
        );
        // Including `d` to `c`, which no walk followed
        assert_eq!(full.edges.len(), 5);
        assert!(
            full.edges
                .iter()
                .any(|e| e.from.letters() == "d" && e.to.letters() == "c")
        );
        assert_eq!(
            full.unmatched,
            [UnmatchedLink {
                from: "e".parse().unwrap(),
                keyword: "zeta".to_string()
            }]
        );

        // All of them down
        let all_down = NoteGraphRequest {
            descendant_depth: None,
            ancestor_depth: Some(0),
            ..graph_request("id=a")
        };
        let (nodes, _, _) = graph(&pool, all_down).await;
        assert_eq!(nodes.len(), 5);
    }
}
