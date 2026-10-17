use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::Arc;

use chrono::NaiveDate;
use chrono::Utc;
use log::info;
use miette::Error;
use miette::Report;
use miette::miette;
use sqlx::QueryBuilder;
use sqlx::Sqlite;
use sqlx::SqlitePool;

use crate::LibraryError;
use crate::api::queue::waiting_on_links;
use crate::config::QueueConfig;
use crate::config::read_external_config;
use crate::helpers::get_or_compile_regex;
use crate::helpers::get_start_end_local_date;
use crate::model::Card;
use crate::model::CardId;
use crate::model::Note;
use crate::model::NoteId;
use crate::model::ReviewLogKind;
use crate::model::StateId;
use crate::parsers::find_parser;
use crate::parsers::get_all_parsers;
use crate::parsers::get_cloze_context_for_card_order;
use crate::schedulers::fsrs::utils::Rating as FsrsRating;
use crate::schedulers::fsrs::utils::State as FsrsState;
use crate::schedulers::fsrs::utils::decay;
use crate::schedulers::fsrs::utils::rating_to_number;
use crate::schedulers::fsrs::utils::state_to_number;
use crate::search::Atom;
use crate::search::Op;
use crate::search::RelativeDate;
use crate::search::Span;
use crate::search::TokenTree;
use crate::search::error_span;
use crate::search::label_error;
use crate::search::lexer::is_quote;
use crate::search::parser::Parser;
use crate::search::relative_date::Anchor;
use crate::search::relative_date::local_day_start_sql;
use crate::search::relative_date::local_month_start_sql;
use crate::search::render_error;

pub(crate) struct Evaluator<'de> {
    query: &'de str,
    parser: Parser<'de>,
    /// Extra SQL condition that every result must satisfy. It is applied inside each limited `or`
    /// branch *before* that branch's LIMIT, so limits count only in-scope items.
    scope: Option<String>,
}

impl<'de> Evaluator<'de> {
    pub(crate) fn new(input: &'de str) -> Self {
        Self {
            query: input,
            parser: Parser::new(input),
            scope: None,
        }
    }

    /// Restricts results to items matching `scope`, a raw SQL condition over the `c`/`n` aliases.
    pub(crate) fn with_scope(mut self, scope: String) -> Self {
        self.scope = Some(scope);
        self
    }

    #[cfg(test)]
    fn evaluate(self, internal_output_type: EvaluatorReturnItemType) -> Result<String, Report> {
        let token_tree = self.parser.parse_expression()?;
        Self::build_query_from_tree(
            &token_tree,
            internal_output_type,
            false,
            self.scope.as_deref(),
        )
    }

    fn build_query_from_tree(
        token_tree: &TokenTree,
        internal_output_type: EvaluatorReturnItemType,
        needs_parser: bool,
        scope: Option<&str>,
    ) -> Result<String, Report> {
        let mut context = EvaluationContext::new(internal_output_type, scope.map(str::to_string));
        context.root_context = true;
        if needs_parser {
            context.table_requirements.needs_parser = true;
        }
        token_tree.evaluate(&mut context)?;
        // Notes found by words come in order of relevance, unless the query sorts them.
        if matches!(
            internal_output_type,
            EvaluatorReturnItemType::Notes | EvaluatorReturnItemType::NoteIds
        ) && context.order_by.is_empty()
            && let Some(order) = relevance_order(token_tree)
        {
            context.order_by.push(order);
        }
        if let Some(scope) = scope {
            // Scoping card fields requires the card table even for note output.
            context.table_requirements.needs_card = true;
            context.add_where_clause(format!("({scope})"));
        }
        context.complete_order_by(internal_output_type);
        Ok(context.build_query(internal_output_type))
    }

    pub(crate) async fn get_notes(
        self,
        db: &SqlitePool,
    ) -> Result<Vec<(Note, String)>, crate::Error> {
        #[derive(sqlx::FromRow)]
        struct EnrichedNote {
            #[sqlx(flatten)]
            note: Note,
            #[sqlx(rename = "name")]
            parser_name: String,
        }
        let query = self.query;
        let token_tree = self
            .parser
            .parse_expression()
            .map_err(search_error(query))?;
        check_references_exist(query, &token_tree, db).await?;
        let query_str = Self::build_query_from_tree(
            &token_tree,
            EvaluatorReturnItemType::Notes,
            true,
            self.scope.as_deref(),
        )
        .map_err(search_error(query))?;
        info!("{}", query_str);
        let enriched_cards: Vec<EnrichedNote> = sqlx::query_as(&query_str)
            .fetch_all(db)
            .await
            .map_err(|e| crate::Error::Sqlx { source: e })?;
        let result = enriched_cards
            .into_iter()
            .map(|x| (x.note, x.parser_name))
            .collect::<Vec<_>>();
        Ok(result)
    }

    pub(crate) async fn get_note_ids(self, db: &SqlitePool) -> Result<Vec<NoteId>, crate::Error> {
        let query = self.query;
        let token_tree = self
            .parser
            .parse_expression()
            .map_err(search_error(query))?;
        check_references_exist(query, &token_tree, db).await?;
        let query_str = Self::build_query_from_tree(
            &token_tree,
            EvaluatorReturnItemType::NoteIds,
            false,
            self.scope.as_deref(),
        )
        .map_err(search_error(query))?;
        info!("{}", query_str);
        let note_ids: Vec<NoteId> = sqlx::query_scalar(&query_str)
            .fetch_all(db)
            .await
            .map_err(|e| crate::Error::Sqlx { source: e })?;
        Ok(note_ids)
    }

    pub(crate) async fn get_cards(
        self,
        db: &SqlitePool,
    ) -> Result<Vec<(Card, String)>, crate::Error> {
        #[derive(sqlx::FromRow)]
        struct EnrichedCard {
            #[sqlx(flatten)]
            card: Card,
            #[sqlx(rename = "name")]
            parser_name: String,
        }
        let query = self.query;

        let token_tree = self
            .parser
            .parse_expression()
            .map_err(search_error(query))?;
        check_references_exist(query, &token_tree, db).await?;

        if has_cloze_field(&token_tree) {
            return Self::get_cards_cloze_path(&token_tree, query, self.scope.as_deref(), db).await;
        }

        let query_str = Self::build_query_from_tree(
            &token_tree,
            EvaluatorReturnItemType::Cards,
            true,
            self.scope.as_deref(),
        )
        .map_err(search_error(query))?;
        info!("{}", query_str);
        let enriched_cards: Vec<EnrichedCard> = sqlx::query_as(&query_str)
            .fetch_all(db)
            .await
            .map_err(|e| crate::Error::Sqlx { source: e })?;
        let result = enriched_cards
            .into_iter()
            .map(|x| (x.card, x.parser_name))
            .collect::<Vec<_>>();
        Ok(result)
    }

    /// The parsed query and the SQL that runs it, as `spares note search --explain` prints them.
    pub(crate) fn explain(
        self,
        output_type: EvaluatorReturnItemType,
    ) -> Result<String, crate::Error> {
        let to_error = search_error(self.query);
        let token_tree = self.parser.parse_expression().map_err(&to_error)?;
        let needs_parser = matches!(
            output_type,
            EvaluatorReturnItemType::Notes | EvaluatorReturnItemType::Cards
        );
        let sql = |tree: &TokenTree| {
            Self::build_query_from_tree(tree, output_type, needs_parser, self.scope.as_deref())
                .map(|sql| format_sql(&sql))
                .map_err(&to_error)
        };
        let mut out = format!("Parsed:\n{token_tree}\n\nSQL:\n");
        let is_cards = matches!(
            output_type,
            EvaluatorReturnItemType::Cards | EvaluatorReturnItemType::CardIds
        );
        if is_cards && has_cloze_field(&token_tree) {
            out.push_str(
                "`c.cloze` is matched after the database query, so each of these runs on its own and \
                 the cards they find are combined:\n",
            );
            for disjunct in to_dnf(token_tree) {
                let _ = write!(out, "\n{}\n", sql(&disjunct)?);
            }
        } else {
            out.push_str(&sql(&token_tree)?);
            out.push('\n');
        }
        Ok(out)
    }

    /// The sort the query asks for with `sort_by_asc` and `sort_by_desc` outside any limited
    /// group, as `ORDER BY` terms (`"<expr> ASC"` or `"<expr> DESC"`) over the `c`, `n` and `p`
    /// aliases, most significant first. A sort inside a limited group, like the one in
    /// `(tag=a sort_by_asc=c.due limit=5)`, only picks the cards that group keeps, so it is left
    /// out.
    pub(crate) fn card_order_by(self) -> Result<Vec<String>, crate::Error> {
        let to_error = search_error(self.query);
        let token_tree = self.parser.parse_expression().map_err(&to_error)?;
        let mut context = EvaluationContext::new(EvaluatorReturnItemType::CardIds, None);
        context.root_context = true;
        token_tree.evaluate(&mut context).map_err(to_error)?;
        Ok(context.order_by)
    }

    pub(crate) async fn get_card_ids(self, db: &SqlitePool) -> Result<Vec<CardId>, crate::Error> {
        let query = self.query;
        let token_tree = self
            .parser
            .parse_expression()
            .map_err(search_error(query))?;
        check_references_exist(query, &token_tree, db).await?;

        if has_cloze_field(&token_tree) {
            return Ok(
                Self::get_cards_cloze_path(&token_tree, query, self.scope.as_deref(), db)
                    .await?
                    .into_iter()
                    .map(|(card, _)| card.id)
                    .collect(),
            );
        }

        let query_str = Self::build_query_from_tree(
            &token_tree,
            EvaluatorReturnItemType::CardIds,
            false,
            self.scope.as_deref(),
        )
        .map_err(search_error(query))?;
        info!("{}", query_str);
        let card_ids: Vec<CardId> = sqlx::query_scalar(&query_str)
            .fetch_all(db)
            .await
            .map_err(|e| crate::Error::Sqlx { source: e })?;
        Ok(card_ids)
    }

    /// Cloze path: convert the query tree to DNF, then for each disjunct run SQL +
    /// optional in-memory cloze filtering, and union the results.
    async fn get_cards_cloze_path(
        token_tree: &TokenTree<'_>,
        query: &str,
        scope: Option<&str>,
        db: &SqlitePool,
    ) -> Result<Vec<(Card, String)>, crate::Error> {
        #[derive(sqlx::FromRow)]
        struct EnrichedCardWithNoteData {
            #[sqlx(flatten)]
            card: Card,
            #[sqlx(rename = "name")]
            parser_name: String,
            note_data: String,
        }

        #[derive(sqlx::FromRow)]
        struct EnrichedCard {
            #[sqlx(flatten)]
            card: Card,
            #[sqlx(rename = "name")]
            parser_name: String,
        }

        let disjuncts = to_dnf(token_tree.clone());
        let mut seen_ids: HashSet<CardId> = HashSet::new();
        let mut result: Vec<(Card, String)> = Vec::new();

        for disjunct in &disjuncts {
            if has_cloze_field(disjunct) {
                // Cloze disjunct: fetch with note_data, then post-filter in memory.
                let cloze_patterns = extract_cloze_patterns(disjunct);
                let query_str = Self::build_query_from_tree(
                    disjunct,
                    EvaluatorReturnItemType::Cards,
                    true,
                    scope,
                )
                .map_err(search_error(query))?;
                info!("{}", query_str);
                let candidates: Vec<EnrichedCardWithNoteData> = sqlx::query_as(&query_str)
                    .fetch_all(db)
                    .await
                    .map_err(|e| crate::Error::Sqlx { source: e })?;
                let all_parsers = get_all_parsers();
                for candidate in candidates {
                    if seen_ids.contains(&candidate.card.id) {
                        continue;
                    }
                    let parser = find_parser(&candidate.parser_name, &all_parsers)?;
                    if let Some(ctx) = get_cloze_context_for_card_order(
                        parser.as_ref(),
                        &candidate.note_data,
                        candidate.card.order,
                    )
                    .map_err(crate::Error::Library)?
                        && cloze_patterns.iter().all(|p| p.matches(&ctx))
                    {
                        seen_ids.insert(candidate.card.id);
                        result.push((candidate.card, candidate.parser_name));
                    }
                }
            } else {
                // No cloze in this disjunct — pure SQL path, no note_data needed.
                let query_str = Self::build_query_from_tree(
                    disjunct,
                    EvaluatorReturnItemType::Cards,
                    true,
                    scope,
                )
                .map_err(search_error(query))?;
                info!("{}", query_str);
                let cards: Vec<EnrichedCard> = sqlx::query_as(&query_str)
                    .fetch_all(db)
                    .await
                    .map_err(|e| crate::Error::Sqlx { source: e })?;
                for c in cards {
                    if seen_ids.insert(c.card.id) {
                        result.push((c.card, c.parser_name));
                    }
                }
            }
        }
        Ok(result)
    }
}

/// Converts an error in `query` to the error returned for a search.
fn search_error(query: &str) -> impl Fn(Report) -> crate::Error + '_ {
    move |e| {
        // Errors found while evaluating mark a place but leave the query's text to this.
        let e = if e.source_code().is_none() {
            e.with_source_code(query.to_string())
        } else {
            e
        };
        crate::Error::Library(LibraryError::Search {
            message: render_error(query, &e),
            span: error_span(query, &e),
        })
    }
}

/// `sql` with each clause of the outermost query on its own line, for reading. Subqueries and
/// strings are left as they are.
fn format_sql(sql: &str) -> String {
    const CLAUSES: [&str; 5] = [" FROM ", " LEFT JOIN ", " WHERE ", " ORDER BY ", " LIMIT "];
    let mut out = String::with_capacity(sql.len());
    let mut depth = 0_usize;
    let mut in_string = false;
    for (i, c) in sql.char_indices() {
        match c {
            '\'' => in_string = !in_string,
            '(' if !in_string => depth += 1,
            ')' if !in_string => depth = depth.saturating_sub(1),
            ' ' if !in_string
                && depth == 0
                && CLAUSES.iter().any(|clause| sql[i..].starts_with(clause)) =>
            {
                out.push('\n');
                continue;
            }
            _ => {}
        }
        out.push(c);
    }
    out
}

/// `s` as an SQL string literal.
fn sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// `s` with the `LIKE` wildcards `%` and `_` escaped, to be matched literally by
/// [`like_pattern`].
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// A `LIKE` pattern for `pattern`, whose literal `%`, `_` and `\` are escaped by [`escape_like`].
fn like_pattern(pattern: &str) -> String {
    if pattern.contains('\\') {
        format!("{} ESCAPE '\\'", sql_string(pattern))
    } else {
        sql_string(pattern)
    }
}

/// The names of the card states, as in `c.state=review`, and the FSRS state each stands for.
const CARD_STATES: [(&str, StateId); 4] = [
    ("new", 0),
    ("learning", 1),
    ("review", 2),
    ("relearning", 3),
];

fn card_state_from_name(name: &str) -> Option<StateId> {
    CARD_STATES
        .iter()
        .find(|(state_name, _)| state_name.eq_ignore_ascii_case(name))
        .map(|(_, state)| *state)
}

/// The card states for errors, like `` `new` (0), `learning` (1), ... ``.
fn card_state_names() -> String {
    let names = CARD_STATES
        .iter()
        .map(|(name, state)| format!("`{name}` ({state})"))
        .collect::<Vec<_>>();
    format!(
        "{} and {}",
        names[..names.len() - 1].join(", "),
        names[names.len() - 1]
    )
}

/// Whether `name` is a boolean field like `c.suspended`, which on its own means it is true.
pub(crate) fn is_boolean_field(name: &str) -> bool {
    Field::from_str(name).is_ok_and(|field| field.get_field_type() == FieldType::Boolean)
}

#[derive(Debug, Clone, PartialEq)]
enum Field {
    Note(NoteField),
    Card(CardField),
    SortAsc(String),
    SortDesc(String),
    Limit,
}

impl Default for Field {
    fn default() -> Self {
        Self::Note(NoteField::default())
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
enum NoteField {
    Id,
    #[default]
    Data,
    CreatedAt,
    UpdatedAt,
    ParserName,
    Tag,
    Keyword,
    CustomData(String),
    LinkedToNote,
    LinkedToKeyword,
    LinkedFromNote,
}

#[derive(Debug, Clone, PartialEq)]
enum CardField {
    Id,
    CreatedAt,
    UpdatedAt,
    Due,
    Stability,
    Difficulty,
    DesiredRetention,
    // SpecialState,
    Suspended,
    UserBuried,
    SchedulerBuried,
    WaitingOnLinks,
    State,
    CustomData(String),
    /// A rating, in reviews at the time written in brackets, like the `>=today-7d` of
    /// `c.rated[>=today-7d]`, if there is one.
    Rated(Option<String>),
    Reviewed,
    Reps,
    Lapses,
    Interval,
    Retrievability,
    /// When a review in `review_log rl` was, for the brackets of `c.rated[…]`. It can't be
    /// written in a query.
    ReviewedAt,
    Count,
    Cloze,
}

/// A field as written in a query, with an example and what it matches. Every field is here, so
/// errors and `spares note search --fields` list them from this table, and the tests check that
/// each one parses.
pub(crate) struct FieldDoc {
    pub(crate) name: &'static str,
    pub(crate) example: &'static str,
    pub(crate) about: &'static str,
}

const fn doc(name: &'static str, example: &'static str, about: &'static str) -> FieldDoc {
    FieldDoc {
        name,
        example,
        about,
    }
}

#[rustfmt::skip]
pub(crate) const FIELDS: [FieldDoc; 35] = [
    doc("id", "id=gir", "The note's id"),
    doc("data", "data~verb", "The note's text, which a word on its own searches"),
    doc("created_at", "created_at>=today-7d", "When the note was created"),
    doc("updated_at", "updated_at>=now-12h", "When the note was last changed"),
    doc("parser_name", "parser_name=markdown", "The parser the note is written for"),
    doc("tag", "tag=spanish", "A tag of the note or card, including its descendants"),
    doc("keyword", "keyword=\"Irregular verbs\"", "A keyword of the note"),
    doc("custom_data", "custom_data.key=1", "A value in the note's custom data"),
    doc("linked_to_note", "linked_to_note=gir", "A note the note links to"),
    doc("linked_to_keyword", "linked_to_keyword=\"Irregular verbs\"", "A keyword the note links to"),
    doc("linked_from_note", "linked_from_note=gir", "A note that links to the note"),
    doc("c.id", "c.id=12", "The card's id"),
    doc("c.created_at", "c.created_at>=today", "When the card was created"),
    doc("c.updated_at", "c.updated_at>=today", "When the card was last changed"),
    doc("c.due", "c.due<=today", "When the card is due"),
    doc("c.stability", "c.stability>=30", "The card's FSRS stability, in days"),
    doc("c.difficulty", "c.difficulty>7", "The card's FSRS difficulty, from 1 to 10"),
    doc("c.desired_retention", "c.desired_retention=0.9", "The retention the card is scheduled for"),
    doc("c.suspended", "c.suspended", "Whether the card is suspended"),
    doc("c.user_buried", "c.user_buried", "Whether the card was buried by hand"),
    doc("c.scheduler_buried", "c.scheduler_buried", "Whether the card was buried as a sibling"),
    doc("c.waiting_on_links", "c.waiting_on_links", "Whether the card waits for the notes it links to"),
    doc("c.state", "c.state=review", "The card's state: new, learning, review or relearning"),
    doc("c.custom_data", "c.custom_data.key=1", "A value in the card's custom data"),
    doc("c.rated", "c.rated[>=today-7d]=1", "A rating the card got in a review, from 1 (again) to 4 (easy), in reviews at the time in brackets if there are any"),
    doc("c.reviewed", "c.reviewed>=today-7d", "When the card was last reviewed"),
    doc("c.reps", "c.reps>=10", "How many times the card was reviewed"),
    doc("c.lapses", "c.lapses>=3", "How many times the card was forgotten after it had been learned"),
    doc("c.interval", "c.interval>=30", "Days from the card's last review to when it is due"),
    doc("c.retrievability", "c.retrievability<0.8", "How likely you are to recall the card now, from 0 to 1, as FSRS predicts"),
    doc("c.count", "c.count>=2", "How many cards the note has"),
    doc("c.cloze", "c.cloze~Paris", "The text of the card's cloze"),
    doc("sort_by_asc", "sort_by_asc=c.due", "Sorts by a field, smallest first"),
    doc("sort_by_desc", "sort_by_desc=created_at", "Sorts by a field, largest first"),
    doc("limit", "limit=10", "Returns at most this many results"),
];

/// The fields whose names are close to `name`, best first, for a "did you mean" hint. A field
/// matches if it is a few edits away, if it starts with `name`, or either of those once its `c.` is
/// left off, so `due` suggests `c.due`.
fn similar_fields(name: &str) -> Vec<&'static str> {
    let name = name.to_lowercase();
    let max_distance = (name.chars().count() / 3).max(1);
    let mut matches = FIELDS
        .iter()
        .filter_map(|field| {
            let short = field.name.strip_prefix("c.").unwrap_or(field.name);
            let distance = edit_distance(&name, field.name).min(edit_distance(&name, short));
            let is_prefix = name.chars().count() >= 3
                && (field.name.starts_with(&name) || short.starts_with(&name));
            (distance <= max_distance || is_prefix).then_some((distance, field.name))
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|(distance, _)| *distance);
    matches.into_iter().map(|(_, name)| name).collect()
}

/// `names` as a list for a sentence, like `` `a`, `b` or `c` ``.
fn or_list(names: &[&str]) -> String {
    let quoted = names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// The Levenshtein distance between `a` and `b`, in characters.
fn edit_distance(a: &str, b: &str) -> usize {
    let b = b.chars().collect::<Vec<_>>();
    let mut previous = (0..=b.len()).collect::<Vec<_>>();
    for (i, a_char) in a.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, b_char) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(a_char != *b_char);
            current.push(substitution.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

/// The SQLite JSON path, like `$.x.y[1]` or `$."a's key"`, of the path written after
/// `custom_data`, like `.x.y[1]` or `["a's key"]`. SQLite only has `.key` and `[n]` steps, so that
/// is all a path can have.
fn json_path(written: &str) -> Result<String, Error> {
    fn push_key(path: &mut String, key: &str) -> Result<(), Error> {
        if key.is_empty() {
            return Err(miette!(
                "A key is missing. Write `.key`, `[0]` or `[\"a key\"]`"
            ));
        }
        if key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            path.push('.');
            path.push_str(key);
        } else if key.contains('"') {
            return Err(miette!("The key `{key}` can't contain `\"`"));
        } else {
            let _ = write!(path, ".\"{key}\"");
        }
        Ok(())
    }
    let mut path = String::from("$");
    let mut rest = written;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('.') {
            let end = after.find(['.', '[']).unwrap_or(after.len());
            push_key(&mut path, &after[..end])?;
            rest = &after[end..];
        } else if let Some(after) = rest.strip_prefix('[') {
            let mut chars = after.char_indices();
            let (key, after_key) = match chars.next() {
                // A quoted key, like `["a's key"]`
                Some((_, open)) if is_quote(open) => {
                    let mut key = String::new();
                    let mut end = None;
                    while let Some((i, c)) = chars.next() {
                        match c {
                            '\\' => key.extend(chars.next().map(|(_, c)| c)),
                            c if c == '"' || (open != '"' && is_quote(c)) => {
                                end = Some(i + c.len_utf8());
                                break;
                            }
                            c => key.push(c),
                        }
                    }
                    let end = end.ok_or_else(|| miette!("A quoted key is never closed"))?;
                    push_key(&mut path, &key)?;
                    (key, &after[end..])
                }
                // An array index, like `[1]`
                _ => {
                    let end = after.find(']').unwrap_or(after.len());
                    let index = &after[..end];
                    if index.is_empty() || !index.chars().all(|c| c.is_ascii_digit()) {
                        return Err(miette!(
                            "`[{index}]` is not an array index. Write a number, like `[0]`, or a key in quotes, like `[\"a key\"]`"
                        ));
                    }
                    let _ = write!(path, "[{index}]");
                    (index.to_string(), &after[end..])
                }
            };
            rest = after_key
                .strip_prefix(']')
                .ok_or_else(|| miette!("Missing `]` after `[{key}`"))?;
        } else {
            return Err(miette!(
                "Unexpected `{rest}`. Write `.key`, `[0]` or `[\"a key\"]`"
            ));
        }
    }
    Ok(path)
}

#[derive(Debug, Clone, PartialEq)]
enum FieldType {
    Integer,
    Float,
    String,
    DateTime,
    Json,
    Boolean,
    Regex,
}

impl FieldType {
    /// How to write a value of the type, for errors.
    fn how_to_write(&self) -> &'static str {
        match self {
            FieldType::Integer => "Write a whole number, like `3`",
            FieldType::Float => "Write a number, like `0.9` or `3`",
            FieldType::String => "Write a word, or text in quotes",
            FieldType::DateTime => "Write a date, like `2026-10-17`, `today+1d` or `now-12h`",
            FieldType::Json => "Write a number, text, `true` or `false`",
            FieldType::Boolean => "Write `true` or `false`",
            FieldType::Regex => "Write a regex, like `re:\"^a\"`",
        }
    }

    /// The kind of value the type holds, for errors, like `a number`.
    fn describe(&self) -> &'static str {
        match self {
            FieldType::Integer => "a whole number",
            FieldType::Float => "a number",
            FieldType::String => "text",
            FieldType::DateTime => "a date",
            FieldType::Json => "JSON",
            FieldType::Boolean => "`true` or `false`",
            FieldType::Regex => "a regex",
        }
    }
}

impl Field {
    fn from_str(value_str: &str) -> Result<Field, Error> {
        if value_str.is_empty() {
            return Ok(Self::default());
        }
        let normalized_value_str = if value_str.starts_with("card.") {
            value_str.replacen("card.", "c.", 1)
        } else {
            value_str.to_string()
        };
        // Ratings in reviews at a time, like `c.rated[>=today-7d]`.
        if let Some(range) = normalized_value_str
            .strip_prefix("c.rated[")
            .and_then(|rest| rest.strip_suffix(']'))
        {
            return Ok(Field::Card(CardField::Rated(Some(range.to_string()))));
        }
        // A path into custom data, like `custom_data.x.y[1]`.
        for (prefix, is_card) in [("custom_data", false), ("c.custom_data", true)] {
            if let Some(path) = normalized_value_str.strip_prefix(prefix)
                && (path.starts_with('.') || path.starts_with('['))
            {
                let path = json_path(path)
                    .map_err(|e| e.wrap_err(format!("in the path of `{value_str}`")))?;
                return Ok(if is_card {
                    Field::Card(CardField::CustomData(path))
                } else {
                    Field::Note(NoteField::CustomData(path))
                });
            }
        }
        match normalized_value_str.as_str() {
            "id" => Ok(Field::Note(NoteField::Id)),
            "data" => Ok(Field::Note(NoteField::Data)),
            "created_at" => Ok(Field::Note(NoteField::CreatedAt)),
            "updated_at" => Ok(Field::Note(NoteField::UpdatedAt)),
            "parser_name" => Ok(Field::Note(NoteField::ParserName)),
            "tag" => Ok(Field::Note(NoteField::Tag)),
            "keyword" => Ok(Field::Note(NoteField::Keyword)),
            "custom_data" => Ok(Field::Note(NoteField::CustomData("$".to_string()))),
            "linked_to_note" => Ok(Field::Note(NoteField::LinkedToNote)),
            "linked_to_keyword" => Ok(Field::Note(NoteField::LinkedToKeyword)),
            "linked_from_note" => Ok(Field::Note(NoteField::LinkedFromNote)),
            "c.id" => Ok(Field::Card(CardField::Id)),
            "c.created_at" => Ok(Field::Card(CardField::CreatedAt)),
            "c.updated_at" => Ok(Field::Card(CardField::UpdatedAt)),
            "c.due" => Ok(Field::Card(CardField::Due)),
            "c.stability" => Ok(Field::Card(CardField::Stability)),
            "c.difficulty" => Ok(Field::Card(CardField::Difficulty)),
            "c.desired_retention" => Ok(Field::Card(CardField::DesiredRetention)),
            // "c.special_state" => Ok(Field::Card(CardField::SpecialState)),
            "c.suspended" => Ok(Field::Card(CardField::Suspended)),
            "c.user_buried" => Ok(Field::Card(CardField::UserBuried)),
            "c.scheduler_buried" => Ok(Field::Card(CardField::SchedulerBuried)),
            "c.waiting_on_links" => Ok(Field::Card(CardField::WaitingOnLinks)),
            "c.state" => Ok(Field::Card(CardField::State)),
            "c.custom_data" => Ok(Field::Card(CardField::CustomData("$".to_string()))),
            "c.rated" => Ok(Field::Card(CardField::Rated(None))),
            "c.reviewed" => Ok(Field::Card(CardField::Reviewed)),
            "c.reps" => Ok(Field::Card(CardField::Reps)),
            "c.lapses" => Ok(Field::Card(CardField::Lapses)),
            "c.interval" => Ok(Field::Card(CardField::Interval)),
            "c.retrievability" => Ok(Field::Card(CardField::Retrievability)),
            "c.count" => Ok(Field::Card(CardField::Count)),
            "c.cloze" => Ok(Field::Card(CardField::Cloze)),
            "sort_by_asc" => Ok(Field::SortAsc(String::new())),
            "sort_by_desc" => Ok(Field::SortDesc(String::new())),
            "limit" => Ok(Field::Limit),
            f => {
                let similar = similar_fields(f);
                let all = FIELDS.iter().map(|field| field.name).collect::<Vec<_>>();
                Err(if similar.is_empty() {
                    miette!(
                        help = format!("The fields are {}", all.join(", ")),
                        "Unknown field `{f}`",
                    )
                } else {
                    miette!(
                        help = "`spares note search --fields` lists the fields",
                        "Unknown field `{f}`: did you mean {}?",
                        or_list(&similar),
                    )
                })
            }
        }
    }

    fn to_sql_str(&self, value_str: &str) -> Result<String, Error> {
        let with_op = move |field_name: &str| format!("{} {}", field_name, value_str);
        // Never NULL, so negating it matches the cards that have no value, like new cards.
        let never_null = move |expr: &str| format!("COALESCE({expr} {value_str}, 0)");
        Ok(match self {
            Field::Note(note_field) => match note_field {
                NoteField::Id => with_op("n.id"),
                NoteField::Data => with_op("n.data"),
                NoteField::CreatedAt => with_op("n.created_at"),
                NoteField::UpdatedAt => with_op("n.updated_at"),
                NoteField::ParserName => with_op("p.name"),
                // The note's tags, or the card's. `EXISTS` is never NULL, so a note without cards
                // can still match, and `-tag=x` matches it when it isn't tagged.
                NoteField::Tag => format!(
                    "(EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND {value_str}) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND {value_str}))"
                ),
                NoteField::Keyword => {
                    format!(
                        "EXISTS (SELECT 1 FROM note_keyword nk WHERE nk.note_id = n.id AND nk.keyword {})",
                        value_str
                    )
                }
                // Never NULL, so `-custom_data.x=1` matches notes without `x`.
                NoteField::CustomData(json_path) => {
                    format!(
                        "COALESCE(json_extract(n.custom_data, {}) {}, 0)",
                        sql_string(json_path),
                        value_str
                    )
                }
                NoteField::LinkedToNote => {
                    format!(
                        "EXISTS (SELECT 1 FROM note_link nl WHERE nl.parent_note_id = n.id AND nl.linked_note_id {})",
                        value_str
                    )
                }
                NoteField::LinkedToKeyword => {
                    format!(
                        "EXISTS (SELECT 1 FROM note_link nl WHERE nl.parent_note_id = n.id AND nl.searched_keyword {})",
                        value_str
                    )
                }
                NoteField::LinkedFromNote => {
                    format!(
                        "EXISTS (SELECT 1 FROM note_link nl WHERE nl.linked_note_id = n.id AND nl.parent_note_id {})",
                        value_str
                    )
                }
            },
            Field::Card(card_field) => match card_field {
                CardField::Id => with_op("c.id"),
                CardField::CreatedAt => with_op("c.created_at"),
                CardField::UpdatedAt => with_op("c.updated_at"),
                CardField::Due => with_op("c.due"),
                CardField::Stability => with_op("c.stability"),
                CardField::Difficulty => with_op("c.difficulty"),
                CardField::DesiredRetention => with_op("c.desired_retention"),
                // CardField::SpecialState => "c.special_state".to_string(),
                CardField::Suspended => {
                    if value_str.contains("false") {
                        "(c.special_state IS NULL OR c.special_state != 1)".to_string()
                    } else {
                        "c.special_state = 1".to_string()
                    }
                }
                CardField::UserBuried => {
                    if value_str.contains("false") {
                        "(c.special_state IS NULL OR c.special_state != 2)".to_string()
                    } else {
                        "c.special_state = 2".to_string()
                    }
                }
                CardField::SchedulerBuried => {
                    if value_str.contains("false") {
                        "(c.special_state IS NULL OR c.special_state != 3)".to_string()
                    } else {
                        "c.special_state = 3".to_string()
                    }
                }
                CardField::WaitingOnLinks => {
                    unreachable!("needs the config, see `waiting_on_links_sql`")
                }
                CardField::State => with_op("c.state"),
                CardField::CustomData(json_path) => {
                    format!(
                        "COALESCE(json_extract(c.custom_data, {}) {}, 0)",
                        sql_string(json_path),
                        value_str
                    )
                }
                CardField::Rated(range) => rated_sql(Some(value_str), range.as_deref())?,
                CardField::Reviewed => never_null(&last_rated_sql()),
                CardField::Reps => with_op(&reps_sql()),
                CardField::Lapses => with_op(&lapses_sql()),
                CardField::Interval => never_null(&interval_sql()),
                // `value_str` is a bound on elapsed days over stability, see
                // `retrievability_bound`.
                CardField::Retrievability => never_null(&elapsed_over_stability_sql()),
                CardField::ReviewedAt => with_op("rl.reviewed_at"),
                CardField::Count => {
                    with_op("(SELECT COUNT(*) FROM card c2 WHERE c2.note_id = n.id)")
                }
                CardField::Cloze => {
                    // Cannot be evaluated in SQL — matched in memory after SQL fetch.
                    // Return a tautology so other SQL conditions still apply.
                    "1".to_string()
                }
            },
            Field::SortAsc(_) | Field::SortDesc(_) | Field::Limit => String::new(),
        })
    }

    /// The kind of value the field holds, as listed by `spares note search --fields`.
    fn kind_name(&self) -> &'static str {
        match self {
            Field::Note(NoteField::Id | NoteField::LinkedToNote | NoteField::LinkedFromNote) => {
                "note id"
            }
            Field::Card(CardField::State) => "card state",
            Field::SortAsc(_) | Field::SortDesc(_) => "field",
            _ => match self.get_field_type() {
                FieldType::Integer => "whole number",
                FieldType::Float => "number",
                FieldType::String => "text",
                FieldType::DateTime => "date",
                FieldType::Json => "JSON",
                FieldType::Boolean => "true or false",
                FieldType::Regex => "regex",
            },
        }
    }

    fn get_field_type(&self) -> FieldType {
        match self {
            Field::Note(note_field) => match note_field {
                NoteField::Id | NoteField::LinkedToNote | NoteField::LinkedFromNote => {
                    FieldType::Integer
                }
                NoteField::Data
                | NoteField::Keyword
                | NoteField::ParserName
                | NoteField::Tag
                | NoteField::LinkedToKeyword => FieldType::String,
                NoteField::CreatedAt | NoteField::UpdatedAt => FieldType::DateTime,
                NoteField::CustomData(_) => FieldType::Json,
            },
            Field::Card(card_field) => match card_field {
                CardField::Id
                | CardField::Rated(_)
                | CardField::State
                | CardField::Count
                | CardField::Reps
                | CardField::Lapses => FieldType::Integer,
                CardField::CreatedAt
                | CardField::UpdatedAt
                | CardField::Due
                | CardField::Reviewed
                | CardField::ReviewedAt => FieldType::DateTime,
                CardField::Stability
                | CardField::Difficulty
                | CardField::DesiredRetention
                | CardField::Interval
                | CardField::Retrievability => FieldType::Float,
                // CardField::SpecialState => {
                CardField::Suspended
                | CardField::UserBuried
                | CardField::SchedulerBuried
                | CardField::WaitingOnLinks => FieldType::Boolean,
                CardField::CustomData(_) => FieldType::Json,
                CardField::Cloze => FieldType::String,
            },
            Field::SortAsc(_) | Field::SortDesc(_) | Field::Limit => FieldType::Integer,
        }
    }

    fn to_order_by_expr(field: &Field) -> Result<String, Error> {
        let field_type = field.get_field_type();
        match field_type {
            FieldType::Integer | FieldType::Float | FieldType::DateTime | FieldType::String => {}
            _ => {
                return Err(miette!("it holds {}", field_type.describe()));
            }
        }
        match field {
            Field::Note(note_field) => match note_field {
                NoteField::Id | NoteField::LinkedToNote | NoteField::LinkedFromNote => Ok("n.id".to_string()),
                NoteField::CreatedAt => Ok("n.created_at".to_string()),
                NoteField::UpdatedAt => Ok("n.updated_at".to_string()),
                NoteField::ParserName => Ok("p.name".to_string()),
                NoteField::Data => Ok("n.data".to_string()),
                NoteField::Keyword => Ok(
                    "(SELECT min(nk.keyword) FROM note_keyword nk WHERE nk.note_id = n.id)"
                        .to_string(),
                ),
                NoteField::Tag => Ok(
                    "(SELECT min(t.name) FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id)"
                        .to_string(),
                ),
                NoteField::LinkedToKeyword => Ok(
                    "(SELECT min(nl.searched_keyword) FROM note_link nl WHERE nl.parent_note_id = n.id)"
                        .to_string(),
                ),
                // Unsupported types
                NoteField::CustomData(_) => Err(miette!("it holds JSON")),
            },
            Field::Card(card_field) => match card_field {
                CardField::Id => Ok("c.id".to_string()),
                CardField::CreatedAt => Ok("c.created_at".to_string()),
                CardField::UpdatedAt => Ok("c.updated_at".to_string()),
                CardField::Due => Ok("c.due".to_string()),
                CardField::Stability => Ok("c.stability".to_string()),
                CardField::Difficulty => Ok("c.difficulty".to_string()),
                CardField::DesiredRetention => Ok("c.desired_retention".to_string()),
                CardField::State => Ok("c.state".to_string()),
                CardField::Count => {
                    Ok("(SELECT COUNT(*) FROM card c2 WHERE c2.note_id = n.id)".to_string())
                }
                CardField::Reviewed => Ok(last_rated_sql()),
                CardField::Reps => Ok(reps_sql()),
                CardField::Lapses => Ok(lapses_sql()),
                CardField::Interval => Ok(interval_sql()),
                // Retrievability falls as elapsed days over stability grow.
                CardField::Retrievability => Ok(format!("-{}", elapsed_over_stability_sql())),
                // Unsupported types
                // A card can have many ratings, so there is none to sort by.
                CardField::Rated(_) => Err(miette!("a card can have many ratings")),
                CardField::ReviewedAt => Err(miette!("it isn't a field of cards")),
                CardField::Suspended
                | CardField::UserBuried
                | CardField::SchedulerBuried
                | CardField::WaitingOnLinks
                | CardField::CustomData(_)
                | CardField::Cloze => Err(miette!("it can't be sorted")),
            },
            Field::SortAsc(_) | Field::SortDesc(_) | Field::Limit => {
                Err(miette!("it isn't a field of notes or cards"))
            }
        }
    }
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
struct TableRequirements {
    needs_parser: bool,
    // needs_tag: bool,
    needs_card: bool,
    needs_review_log: bool,
    needs_note_link: bool,
    /// Set when a `c.cloze` condition is present; causes `build_query` to include
    /// `n.data AS note_data` in the `Cards` SELECT for in-memory cloze filtering.
    needs_note_data: bool,
}

impl TableRequirements {
    fn analyze_field(field: Field) -> TableRequirements {
        let mut req = TableRequirements::default();
        match field {
            Field::Note(note_field) => match note_field {
                NoteField::ParserName => req.needs_parser = true,
                // NoteField::Tag => req.needs_tag = true,
                NoteField::Tag => req.needs_card = true,
                NoteField::LinkedToNote
                | NoteField::LinkedToKeyword
                | NoteField::LinkedFromNote => req.needs_note_link = true,
                _ => {}
            },
            Field::Card(card_field) => {
                req.needs_card = true;
                if matches!(card_field, CardField::Cloze) {
                    req.needs_note_data = true;
                }
            }
            // Sort fields don't have requirements until we know the target field name,
            // which is determined during evaluation, not parsing
            Field::SortAsc(_) | Field::SortDesc(_) | Field::Limit => {}
        }
        req
    }

    fn merge(&mut self, other: &TableRequirements) {
        self.needs_parser |= other.needs_parser;
        // self.needs_tag |= other.needs_tag;
        self.needs_card |= other.needs_card;
        self.needs_review_log |= other.needs_review_log;
        self.needs_note_link |= other.needs_note_link;
        self.needs_note_data |= other.needs_note_data;
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum EvaluatorReturnItemType {
    Notes,
    NoteIds,
    Cards,
    CardIds,
}

struct EvaluationContext<'a> {
    query_builder: QueryBuilder<'a, Sqlite>,
    params: Vec<String>,
    field: Option<Field>,
    value_type: Option<FieldType>,
    table_requirements: TableRequirements,
    where_clauses: Vec<String>,
    root_context: bool,
    order_by: Vec<String>,
    limit: Option<u64>,
    /// The item type of the outermost query. Limited `or` branches select this item type's ids.
    output_type: EvaluatorReturnItemType,
    /// See [`Evaluator::scope`].
    scope_clause: Option<String>,
}

impl EvaluationContext<'_> {
    fn new(output_type: EvaluatorReturnItemType, scope_clause: Option<String>) -> Self {
        Self {
            query_builder: QueryBuilder::new(""),
            params: Vec::new(),
            field: None,
            value_type: None,
            table_requirements: TableRequirements::default(),
            where_clauses: Vec::new(),
            root_context: false,
            order_by: Vec::new(),
            limit: None,
            output_type,
            scope_clause,
        }
    }

    /// A fresh context for evaluating a subtree of this one's expression.
    fn child(&self) -> Self {
        Self::new(self.output_type, self.scope_clause.clone())
    }

    /// Compiles this context, which holds an `or` branch that set its own `limit`, into an
    /// `id IN (subquery)` clause, so the limit applies to that branch alone.
    fn into_limited_branch_clause(mut self) -> Result<String, Error> {
        if self.table_requirements.needs_note_data {
            return Err(miette!(
                "c.cloze cannot be used inside a limited group. Cloze matching happens after the database query, so it can't be limited per group."
            ));
        }
        if let Some(scope) = self.scope_clause.clone() {
            self.table_requirements.needs_card = true;
            self.add_where_clause(format!("({scope})"));
        }
        self.complete_order_by(self.output_type);
        let (id_column, subquery_type) = match self.output_type {
            EvaluatorReturnItemType::Notes | EvaluatorReturnItemType::NoteIds => {
                ("n.id", EvaluatorReturnItemType::NoteIds)
            }
            EvaluatorReturnItemType::Cards | EvaluatorReturnItemType::CardIds => {
                ("c.id", EvaluatorReturnItemType::CardIds)
            }
        };
        Ok(format!(
            "{id_column} IN ({})",
            self.build_query(subquery_type)
        ))
    }

    /// Makes the order of `output_type` results deterministic, so that a limit keeps the same
    /// items each time. The query's sort is followed by the id, and without a sort, cards come in
    /// the order review shows them by default (the cards due the longest first) and notes by id.
    fn complete_order_by(&mut self, output_type: EvaluatorReturnItemType) {
        let (default, id) = match output_type {
            EvaluatorReturnItemType::Notes | EvaluatorReturnItemType::NoteIds => {
                (&["n.id ASC"][..], "n.id")
            }
            EvaluatorReturnItemType::Cards | EvaluatorReturnItemType::CardIds => {
                (&["c.due ASC", "n.created_at ASC", "c.id ASC"][..], "c.id")
            }
        };
        if self.order_by.is_empty() {
            self.order_by
                .extend(default.iter().map(ToString::to_string));
        } else if !self
            .order_by
            .iter()
            .any(|term| term.strip_suffix(" ASC").or(term.strip_suffix(" DESC")) == Some(id))
        {
            self.order_by.push(format!("{id} ASC"));
        }
    }

    fn build_query(&mut self, output_type: EvaluatorReturnItemType) -> String {
        // Start with base table
        match output_type {
            EvaluatorReturnItemType::Notes => {
                self.query_builder
                    .push("SELECT DISTINCT n.*, p.name FROM note n");
            }
            EvaluatorReturnItemType::NoteIds => {
                self.query_builder.push("SELECT DISTINCT n.id FROM note n");
            }
            EvaluatorReturnItemType::Cards => {
                if self.table_requirements.needs_note_data {
                    self.query_builder
                        .push("SELECT DISTINCT c.*, p.name, n.data AS note_data FROM card c");
                } else {
                    self.query_builder
                        .push("SELECT DISTINCT c.*, p.name FROM card c");
                }
                self.query_builder
                    .push(" LEFT JOIN note n ON n.id = c.note_id");
            }
            EvaluatorReturnItemType::CardIds => {
                self.query_builder.push("SELECT DISTINCT c.id FROM card c");
                self.query_builder
                    .push(" LEFT JOIN note n ON n.id = c.note_id");
            }
        }

        // Add necessary JOINs based on requirements
        if self.table_requirements.needs_card
            && !matches!(
                output_type,
                EvaluatorReturnItemType::Cards | EvaluatorReturnItemType::CardIds
            )
        {
            self.query_builder
                .push(" LEFT JOIN card c ON n.id = c.note_id");
        }
        if self.table_requirements.needs_parser {
            self.query_builder
                .push(" LEFT JOIN parser p ON n.parser_id = p.id");
        }
        // if self.table_requirements.needs_tag {
        //     self.query_builder
        //         .push(" LEFT JOIN note_tag nt ON n.id = nt.note_id")
        //         .push(" LEFT JOIN tag t ON nt.tag_id = t.id");
        // }
        if self.table_requirements.needs_note_link {
            self.query_builder
                .push(" LEFT JOIN note_link nl ON n.id = nl.parent_note_id");
        }

        // Add WHERE clause if we have conditions
        if !self.where_clauses.is_empty() {
            self.query_builder.push(" WHERE ");
            for (i, clause) in self.where_clauses.iter().enumerate() {
                if i > 0 {
                    self.query_builder.push(" AND ");
                }
                self.query_builder.push(clause);
            }
        }

        if !self.order_by.is_empty() {
            self.query_builder.push(" ORDER BY ");
            for (i, ob) in self.order_by.iter().enumerate() {
                if i > 0 {
                    self.query_builder.push(", ");
                }
                self.query_builder.push(ob);
            }
        }

        if let Some(limit) = self.limit {
            self.query_builder.push(format!(" LIMIT {}", limit));
        }

        self.query_builder.sql().to_string()
    }

    fn add_where_clause(&mut self, clause: String) {
        self.where_clauses.push(clause);
    }
}

trait Evaluate {
    fn evaluate(&self, context: &mut EvaluationContext) -> Result<(), Error>;
}

impl Evaluate for TokenTree<'_> {
    fn evaluate(&self, context: &mut EvaluationContext) -> Result<(), Error> {
        let requirements = self.analyze_requirements()?;
        context.table_requirements.merge(&requirements);
        match self {
            TokenTree::Atom(Atom::Field(name), span) => {
                evaluate_bare_field(name, context).map_err(|e| label_error(e, *span, "this field"))
            }
            TokenTree::Atom(atom, _) => atom.evaluate(context),
            TokenTree::Cons(op, trees) => match op {
                Op::And | Op::Or | Op::Group => {
                    let mut clauses = Vec::new();
                    if trees
                        .iter()
                        .any(|tree| matches!(tree, TokenTree::Atom(Atom::Nil, _)))
                    {
                        return Err(miette!("Found nil atom inside op."));
                    }
                    for tree in trees {
                        let mut inner_context = context.child();
                        tree.evaluate(&mut inner_context)?;
                        // A parenthesized `or` branch with its own limit, like
                        // `(tag=a limit=5) or (tag=b limit=10)`, becomes a self-contained
                        // subquery. Any sort inside it decides which items the limit keeps.
                        if matches!(op, Op::Or) && inner_context.limit.is_some() {
                            if !matches!(tree, TokenTree::Cons(Op::Group, _)) {
                                return Err(label_error(
                                    miette!(
                                        "A limit inside an 'or' must be wrapped in parentheses with the conditions it limits, e.g. `(tag=a limit=5) or (tag=b limit=10)`."
                                    ),
                                    tree.span(),
                                    "this has a limit",
                                ));
                            }
                            context
                                .table_requirements
                                .merge(&inner_context.table_requirements);
                            clauses.push(inner_context.into_limited_branch_clause()?);
                            continue;
                        }
                        // Cannot use sort operations with 'or' operator
                        if matches!(op, Op::Or) && !inner_context.order_by.is_empty() {
                            return Err(label_error(
                                miette!(
                                    "Cannot use sort operations with 'or' operator without a limit. Sort operations cannot be used in alternative conditions unless the alternative is a parenthesized group with its own limit."
                                ),
                                tree.span(),
                                "this has a sort",
                            ));
                        }
                        if let Some(clause) = inner_context.where_clauses.first() {
                            clauses.push(clause.clone());
                        }
                        // Merge order_by clauses
                        // This is necessary to not allow sort by conditions
                        // inside groups "c.stability>=2 or (a sort_by_asc=linked_to_note)"
                        context.order_by.extend(inner_context.order_by);
                        // Merge limit
                        if let Some(inner_limit) = inner_context.limit {
                            if context.limit.is_some() {
                                return Err(label_error(
                                    miette!("limit can only be specified once"),
                                    tree.span(),
                                    "another limit",
                                ));
                            }
                            context.limit = Some(inner_limit);
                        }
                        // Merge table requirements
                        context
                            .table_requirements
                            .merge(&inner_context.table_requirements);
                    }
                    let join_op = match op {
                        Op::And => " AND ",
                        Op::Or => " OR ",
                        Op::Group => " ",
                        _ => unreachable!("by outer match"),
                    };
                    if !clauses.is_empty() {
                        let clause_str = clauses.join(join_op);
                        // Only wrap in parentheses if there are multiple clauses
                        if clauses.len() > 1 {
                            context.add_where_clause(format!("({})", clause_str));
                        } else {
                            context.add_where_clause(clause_str);
                        }
                    }
                    Ok(())
                }
                Op::Minus => evaluate_minus(trees, context)
                    .map_err(|e| label_error(e, self.span(), "this is negated")),
                Op::GreaterThan
                | Op::GreaterThanEqual
                | Op::LessThan
                | Op::LessThanEqual
                | Op::Equal
                | Op::Tilde => evaluate_field_value(trees, context, *op, self.span()),
            },
        }
    }
}

impl TokenTree<'_> {
    fn analyze_requirements(&self) -> Result<TableRequirements, Error> {
        match self {
            TokenTree::Atom(Atom::Field(field), span) => Ok(TableRequirements::analyze_field(
                Field::from_str(field).map_err(|e| label_error(e, *span, "this field"))?,
            )),
            // A query in parentheses as a value, as in `linked_to_note=(c.state=new)`, is its own
            // query with its own tables.
            TokenTree::Cons(
                Op::Equal
                | Op::GreaterThan
                | Op::GreaterThanEqual
                | Op::LessThan
                | Op::LessThanEqual
                | Op::Tilde,
                trees,
            ) => trees.first().map_or_else(
                || Ok(TableRequirements::default()),
                Self::analyze_requirements,
            ),
            TokenTree::Cons(_, trees) => {
                let mut requirements = TableRequirements::default();
                for tree in trees {
                    requirements.merge(&tree.analyze_requirements()?);
                }
                Ok(requirements)
            }
            TokenTree::Atom(_, _) => Ok(TableRequirements::default()),
        }
    }
}

fn evaluate_minus(trees: &[TokenTree], context: &mut EvaluationContext) -> Result<(), Error> {
    if trees.len() != 1 {
        return Err(miette!("Minus operation requires exactly one operand"));
    }
    let mut inner_context = context.child();
    if !matches!(
        trees[0],
        TokenTree::Atom(Atom::Field(_), _)
            | TokenTree::Cons(
                Op::GreaterThan
                    | Op::GreaterThanEqual
                    | Op::LessThan
                    | Op::LessThanEqual
                    | Op::Equal
                    | Op::Tilde
                    | Op::Group
                    | Op::And
                    | Op::Or,
                _
            )
    ) {
        return Err(miette!("Minus operation requires a comparison operand."));
    }
    trees[0].evaluate(&mut inner_context)?;
    // Cannot negate sort operations
    if !inner_context.order_by.is_empty() {
        return Err(miette!(
            "Cannot negate sort operations. Sort operations cannot be used with the minus operator."
        ));
    }
    // Cannot negate limit
    if inner_context.limit.is_some() {
        return Err(miette!(
            "Cannot negate limit. Limit cannot be used with the minus operator."
        ));
    }
    if let Some(clause) = inner_context.where_clauses.first() {
        context.add_where_clause(format!("NOT ({})", clause));
    }
    // Merge order_by clauses (should be empty due to check above, but merge anyway)
    // context.order_by.extend(inner_context.order_by);
    context
        .table_requirements
        .merge(&inner_context.table_requirements);
    Ok(())
}

/// A field on its own, which is a condition only for a custom data path like `custom_data.flag`:
/// the key is there and its value is not `false` or `null`. A boolean field on its own, like
/// `c.suspended`, was already given `=true` by the lexer.
fn evaluate_bare_field(name: &str, context: &mut EvaluationContext) -> Result<(), Error> {
    let (column, path) = match Field::from_str(name)? {
        Field::Note(NoteField::CustomData(path)) => ("n.custom_data", path),
        Field::Card(CardField::CustomData(path)) => ("c.custom_data", path),
        // Any rating at the time, as in `c.rated[today]`.
        Field::Card(CardField::Rated(Some(range))) => {
            context.add_where_clause(rated_sql(None, Some(&range))?);
            return Ok(());
        }
        _ => {
            let example = field_example(name).unwrap_or("tag=spanish");
            return Err(miette!(
                "`{name}` needs a comparison and a value, as in `{example}`"
            ));
        }
    };
    // `json_type` is NULL for a missing key, so a missing key doesn't match, and `-custom_data.x`
    // does.
    context.add_where_clause(format!(
        "COALESCE(json_type({column}, {}) NOT IN ('false', 'null'), 0)",
        sql_string(&path)
    ));
    Ok(())
}

fn validate_field_value_types(
    field_name: &str,
    field_type: &FieldType,
    value_type: &FieldType,
    value: &TokenTree,
) -> Result<(), Error> {
    match (field_type, value_type) {
        (FieldType::Integer, FieldType::Integer)
        | (FieldType::Float, FieldType::Float | FieldType::Integer)
        | (FieldType::String | FieldType::Json, _)
        | (FieldType::DateTime, FieldType::DateTime)
        | (FieldType::Boolean, FieldType::Boolean) => Ok(()),
        _ => {
            let value = match value {
                TokenTree::Atom(Atom::String(s) | Atom::Word(s), _) => s.to_string(),
                value => value.to_string(),
            };
            let example = if matches!(field_type, FieldType::Boolean) {
                Some(format!("{field_name}=false"))
            } else {
                field_example(field_name).map(str::to_string)
            };
            let help = match example {
                Some(example) => format!("{}, as in `{example}`", field_type.how_to_write()),
                None => field_type.how_to_write().to_string(),
            };
            Err(miette!(
                help = help,
                "`{field_name}` holds {}, so it can't be compared with {} `{value}`",
                field_type.describe(),
                value_type.describe(),
            ))
        }
    }
}

/// The names of the fields that `sort_by_asc` and `sort_by_desc` accept.
fn sortable_fields() -> Vec<&'static str> {
    FIELDS
        .iter()
        .map(|doc| doc.name)
        .filter(|name| {
            Field::from_str(name).is_ok_and(|field| Field::to_order_by_expr(&field).is_ok())
        })
        .collect()
}

/// The fields that queries can use, with the kind of value each holds, from [`FIELDS`].
pub(crate) fn field_infos() -> Vec<crate::search::FieldInfo> {
    FIELDS
        .iter()
        .map(|doc| {
            let field = Field::from_str(doc.name);
            crate::search::FieldInfo {
                name: doc.name,
                kind: field.as_ref().map_or("", Field::kind_name),
                example: doc.example,
                about: doc.about,
                sortable: field.is_ok_and(|field| Field::to_order_by_expr(&field).is_ok()),
            }
        })
        .collect()
}

/// The example of the field written `field_name` in [`FIELDS`], if it has one.
fn field_example(field_name: &str) -> Option<&'static str> {
    let name = match field_name.strip_prefix("card.") {
        Some(rest) => format!("c.{rest}"),
        None => field_name.to_string(),
    };
    FIELDS
        .iter()
        .find(|field| field.name == name)
        .map(|field| field.example)
}

fn validate_field_operator_types(
    field_name: &str,
    field_type: &FieldType,
    op: Op,
) -> Result<(), Error> {
    match (field_type, op) {
        (
            FieldType::String | FieldType::Boolean,
            Op::LessThan | Op::LessThanEqual | Op::GreaterThan | Op::GreaterThanEqual,
        )
        | (FieldType::Integer | FieldType::Float | FieldType::Boolean, Op::Tilde) => Err(miette!(
            help = match (field_type, field_example(field_name)) {
                (FieldType::String, _) =>
                    "Use `=` to match it exactly or `~` to search within it".to_string(),
                (FieldType::Boolean, _) => format!("Use `=`, as in `{field_name}=true`"),
                (_, Some(example)) => format!("Use `=`, `<`, `<=`, `>` or `>=`, as in `{example}`"),
                (_, None) => "Use `=`, `<`, `<=`, `>` or `>=`".to_string(),
            },
            "`{field_name}` holds {}, so it can't use `{op}`",
            field_type.describe(),
        )),
        _ => Ok(()),
    }
}

/// Adds the condition that `trees`, a field and a value, compare by `op`. `span` is where the
/// comparison is written, for errors.
fn evaluate_field_value(
    trees: &[TokenTree],
    context: &mut EvaluationContext,
    op: Op,
    span: Span,
) -> Result<(), Error> {
    if trees.len() != 2 {
        return Err(miette!(
            "Field-value operation requires exactly two operands"
        ));
    }

    // First tree should be the field
    let mut field_context = context.child();
    let TokenTree::Atom(field_atom @ Atom::Field(_), _) = &trees[0] else {
        return Err(miette!("An operator requires a field to operate on."));
    };
    field_atom
        .evaluate(&mut field_context)
        .map_err(|e| label_error(e, trees[0].span(), "this field"))?;
    let field = field_context
        .field
        .ok_or_else(|| miette!("Missing field"))?;
    // The field as written, for errors.
    let field_name = trees[0].to_string();
    if let TokenTree::Cons(Op::Group, query) = &trees[1] {
        return evaluate_link_query(&field, &field_name, query, context, op)
            .map_err(|e| label_error(e, span, "this comparison"));
    }
    evaluate_comparison(&field, &field_name, &trees[1], context, op, span)
        // Errors in the operator mark the whole comparison, and the rest the value.
        .map_err(|e| label_error(e, trees[1].span(), "this value"))
}

/// Adds the condition that the note links to, or is linked from, a note matching `query`, as in
/// `linked_to_note=(tag=history)`. `query` always searches notes.
fn evaluate_link_query(
    field: &Field,
    field_name: &str,
    query: &[TokenTree],
    context: &mut EvaluationContext,
    op: Op,
) -> Result<(), Error> {
    let (this_note, other_note) = match field {
        Field::Note(NoteField::LinkedToNote) => ("parent_note_id", "linked_note_id"),
        Field::Note(NoteField::LinkedFromNote) => ("linked_note_id", "parent_note_id"),
        _ => {
            return Err(miette!(
                help = "Only `linked_to_note` and `linked_from_note` take a query, as in `linked_to_note=(tag=history)`",
                "`{field_name}` can't be compared with a query in parentheses",
            ));
        }
    };
    if !matches!(op, Op::Equal) {
        return Err(miette!(
            "A query in parentheses can only follow `=` or `!=`, as in `{field_name}=(tag=history)`"
        ));
    }
    let [query] = query else {
        return Err(miette!("Missing query"));
    };
    if matches!(query, TokenTree::Atom(Atom::Nil, _)) {
        return Err(miette!(
            "The parentheses after `{field_name}=` are empty. Write a query in them, as in `{field_name}=(tag=history)`"
        ));
    }
    if tree_has_modifier(query) {
        return Err(miette!(
            "A sort or limit can't go in `{field_name}=(…)`, since it only says which notes are linked"
        ));
    }
    if has_cloze_field(query) {
        return Err(miette!(
            "`c.cloze` can't go in `{field_name}=(…)`, since it is matched after the database query"
        ));
    }
    let mut query_context = EvaluationContext::new(EvaluatorReturnItemType::NoteIds, None);
    query.evaluate(&mut query_context)?;
    let notes = query_context.build_query(EvaluatorReturnItemType::NoteIds);
    context.add_where_clause(format!(
        "EXISTS (SELECT 1 FROM note_link nl WHERE nl.{this_note} = n.id AND nl.{other_note} IN ({notes}))"
    ));
    Ok(())
}

/// Whether `tree` has a `sort_by_asc`, `sort_by_desc` or `limit` anywhere.
fn tree_has_modifier(tree: &TokenTree) -> bool {
    match tree {
        TokenTree::Atom(Atom::Field(f), _) => {
            matches!(f.as_ref(), "sort_by_asc" | "sort_by_desc" | "limit")
        }
        TokenTree::Atom(_, _) => false,
        TokenTree::Cons(_, children) => children.iter().any(tree_has_modifier),
    }
}

/// Adds the condition that `field` compares with `value` by `op`. `field_name` is the field as
/// written and `comparison_span` where the comparison is, for errors.
#[expect(clippy::too_many_lines)]
fn evaluate_comparison(
    field: &Field,
    field_name: &str,
    value: &TokenTree,
    context: &mut EvaluationContext,
    op: Op,
    comparison_span: Span,
) -> Result<(), Error> {
    let field_type = field.get_field_type();

    let mut value_context = context.child();
    value.evaluate(&mut value_context)?;
    let mut value_type = value_context
        .value_type
        .ok_or_else(|| miette!("Missing value"))?;

    // `today` and `now` are ordinary words unless compared against a date field, so `data~today`
    // still searches for the word.
    let mut relative_date = match value {
        TokenTree::Atom(Atom::RelativeDate(date), _) => Some(date.clone()),
        _ => None,
    };
    if matches!(field_type, FieldType::DateTime)
        && let TokenTree::Atom(Atom::String(s) | Atom::Word(s), _) = value
        && RelativeDate::starts_with_word_anchor(s)
    {
        let date = RelativeDate::parse(s)?;
        value_context.params = vec![date.to_sql()];
        value_type = FieldType::DateTime;
        relative_date = Some(date);
    }
    if relative_date.is_some() && matches!(op, Op::Tilde) {
        return Err(miette!(
            "A relative date cannot be used with operator `{}`. Compare it with `=`, `<`, `<=`, `>` or `>=`.",
            op
        ));
    }
    // A month, like `2026-10`, or a year, like `2026`, compared against a date field, as its first
    // day and how many months it lasts.
    let month_or_year = match value {
        _ if !matches!(field_type, FieldType::DateTime) => None,
        TokenTree::Atom(Atom::String(s) | Atom::Word(s), _)
            if s.len() == 7
                && s.char_indices()
                    .all(|(i, c)| if i == 4 { c == '-' } else { c.is_ascii_digit() }) =>
        {
            let month = NaiveDate::parse_from_str(&format!("{s}-01"), "%Y-%m-%d")
                .map_err(|_| miette!("`{s}` is not a month"))?;
            Some((month, 1))
        }
        TokenTree::Atom(Atom::Integer(year), _) if (1000..=9999).contains(year) => {
            i32::try_from(*year)
                .ok()
                .and_then(|year| NaiveDate::from_ymd_opt(year, 1, 1))
                .map(|start| (start, 12))
        }
        _ => None,
    };
    if month_or_year.is_some() {
        value_type = FieldType::DateTime;
    }
    // A day, like `2026-10-17` or `today-1d`, a month or a year compared against a date field
    // stands for the whole local period, so `created_at=2026-10-17` finds notes created any time
    // that day and `created_at=2026` any time that year. Against a text field, it is just the
    // text.
    let period = match (value, &relative_date, month_or_year) {
        (TokenTree::Atom(Atom::Date(_), _), _, _) | (_, _, Some(_))
            if matches!(field_type, FieldType::DateTime) && matches!(op, Op::Tilde) =>
        {
            return Err(miette!(
                "A date cannot be used with operator `{}`. Compare it with `=`, `<`, `<=`, `>` or `>=`.",
                op
            ));
        }
        (_, _, Some((start, months))) => Some((
            local_month_start_sql(start, 0),
            local_month_start_sql(start, months),
        )),
        (TokenTree::Atom(Atom::Date(date), _), _, _)
            if matches!(field_type, FieldType::DateTime) =>
        {
            Some((local_day_start_sql(*date, 0), local_day_start_sql(*date, 1)))
        }
        (TokenTree::Atom(Atom::Date(date), _), _, _) => {
            value_context.params = vec![date.to_string()];
            value_type = FieldType::String;
            None
        }
        (_, Some(date), _) => date.next_day().map(|next| (date.to_sql(), next.to_sql())),
        _ => None,
    };

    // Card states can be written by name, as in `c.state=review`.
    if matches!(field, Field::Card(CardField::State)) && matches!(value_type, FieldType::String) {
        let name = value_context
            .params
            .pop()
            .ok_or_else(|| miette!("Missing value"))?;
        let state = card_state_from_name(&name).ok_or_else(|| {
            miette!(
                help = format!("The states are {}", card_state_names()),
                "`{name}` is not a card state",
            )
        })?;
        value_context.params.push(state.to_string());
        value_type = FieldType::Integer;
    }
    // The state's name goes in the SQL, so `--explain` shows `c.state = 2 /* review */`.
    if matches!(field, Field::Card(CardField::State))
        && matches!(value_type, FieldType::Integer)
        && let Some(state) = value_context.params.last_mut()
        && let Some((name, _)) = CARD_STATES.iter().find(|(_, id)| id.to_string() == *state)
    {
        let _ = write!(state, " /* {name} */");
    }

    // Note ids are written as letters and stored packed, which keeps their order, so a letter id
    // becomes its packed integer and is then compared like any integer. Numbers are rejected, so
    // that an old query like `id>100` cannot silently change meaning.
    if matches!(
        field,
        Field::Note(NoteField::Id | NoteField::LinkedToNote | NoteField::LinkedFromNote)
    ) && !matches!(op, Op::Tilde)
    {
        let value = value_context
            .params
            .pop()
            .ok_or_else(|| miette!("Missing value"))?;
        if !matches!(value_type, FieldType::String | FieldType::Integer) {
            return Err(miette!("`{value}` is not a note id"));
        }
        let note_id = value.parse::<NoteId>().map_err(|e| miette!("{e}"))?;
        value_context.params.push(note_id.packed().to_string());
        value_type = FieldType::Integer;
    }

    // Special-case sorting keys
    if matches!(field, Field::SortAsc(_) | Field::SortDesc(_)) {
        if !matches!(op, Op::Equal) {
            return Err(miette!(
                "Sorting requires '=' operator, e.g., sort_by_asc=created_at"
            ));
        }
        let rhs = value_context
            .params
            .first()
            .ok_or_else(|| miette!("Missing sort field name"))?
            .clone();
        // Several fields, like `sort_by_asc=c.due,c.stability`, sort by each in turn.
        for rhs_field_name in rhs.split(',').map(str::trim) {
            if rhs_field_name.is_empty() {
                return Err(miette!(
                    "Missing a field in `{rhs}`. Separate fields with `,`, as in `sort_by_asc=c.due,c.stability`"
                ));
            }
            let target_field = Field::from_str(rhs_field_name)?;
            let order_expr = Field::to_order_by_expr(&target_field).map_err(|e| {
                miette!(
                    help = format!(
                        "The fields that can be sorted are {}",
                        sortable_fields().join(", ")
                    ),
                    "Can't sort by `{rhs_field_name}`, since {e}",
                )
            })?;
            // Merge table requirements for the sort target field
            context
                .table_requirements
                .merge(&TableRequirements::analyze_field(target_field.clone()));
            // Cards with no value, like new cards for `c.retrievability`, come last either way.
            if matches!(
                target_field,
                Field::Card(CardField::Reviewed | CardField::Interval | CardField::Retrievability)
            ) {
                context.order_by.push(format!("({order_expr}) IS NULL ASC"));
            }
            match field {
                Field::SortAsc(_) => context.order_by.push(format!("{order_expr} ASC")),
                Field::SortDesc(_) => context.order_by.push(format!("{order_expr} DESC")),
                _ => {}
            }
        }
        return Ok(());
    }

    // Special-case limit
    if matches!(field, Field::Limit) {
        if !matches!(op, Op::Equal) {
            return Err(miette!("Limit requires '=' operator, e.g., limit=100"));
        }
        if context.limit.is_some() {
            return Err(miette!("limit can only be specified once"));
        }
        let limit_value = value_context
            .params
            .first()
            .ok_or_else(|| miette!("Missing limit value"))?
            .parse::<u64>()
            .map_err(|_| miette!("Limit value must be a positive integer"))?;
        context.limit = Some(limit_value);
        return Ok(());
    }
    match op {
        Op::Equal | Op::GreaterThan | Op::GreaterThanEqual | Op::LessThan | Op::LessThanEqual => {
            if matches!(value_type, FieldType::Regex) {
                return Err(miette!(
                    "A regex string cannot be used with operator `{}`. Try changing the operator to `{}` or changing the regex string to a string.",
                    op,
                    Op::Tilde
                ));
            }
            if let Some(mut value_param) = value_context.params.pop() {
                if let Some((start, end)) = &period {
                    value_context.params.push(match op {
                        Op::Equal => format!("BETWEEN {start} AND ({end} - 1)"),
                        Op::GreaterThan => format!(">= {end}"),
                        Op::GreaterThanEqual => format!(">= {start}"),
                        Op::LessThan => format!("< {start}"),
                        Op::LessThanEqual => format!("< {end}"),
                        _ => unreachable!("by the outer match"),
                    });
                } else if matches!(field, Field::Note(NoteField::Tag)) && matches!(op, Op::Equal) {
                    // The tag and its descendants, like `history` and `history:rome`.
                    let name = escape_like(&value_param);
                    value_context.params.push(format!(
                        "(t.name LIKE {} OR t.name LIKE {})",
                        like_pattern(&name),
                        like_pattern(&format!("{name}:%")),
                    ));
                } else if matches!(field, Field::Card(CardField::Retrievability))
                    && let Ok(value) = value_param.parse::<f64>()
                {
                    value_context.params.push(retrievability_bound(op, value)?);
                } else if matches!(field_type, FieldType::Float)
                    && matches!(op, Op::Equal)
                    && let Ok(value) = value_param.parse::<f64>()
                {
                    // Stored numbers may have passed through `f32`, so `0.9` can be stored as
                    // `0.8999999761581421`. Equal means within a millionth of the value.
                    let tolerance = 1e-6 * value.abs().max(1.0);
                    value_context.params.push(format!(
                        "BETWEEN {} AND {}",
                        value - tolerance,
                        value + tolerance
                    ));
                } else {
                    if matches!(field_type, FieldType::String)
                        || (matches!(field_type, FieldType::Json)
                            && matches!(value_type, FieldType::String))
                    {
                        value_param = sql_string(&value_param);
                    }
                    value_context.params.push(format!("{} {}", op, value_param));
                }
            }
        }
        Op::Tilde => {
            if let Some(value_param) = value_context.params.pop() {
                if matches!(value_type, FieldType::Regex) {
                    value_context
                        .params
                        .push(format!("REGEXP {}", sql_string(&value_param)));
                } else {
                    // A value without quotes, like a word or a date, searches the text for words
                    // starting with it, so `cat` finds `cats` but not `concatenate`. In quotes,
                    // it can be anywhere.
                    let word_start = !matches!(value, TokenTree::Atom(Atom::String(_), _))
                        && matches!(
                            field,
                            Field::Note(NoteField::Data) | Field::Card(CardField::Cloze)
                        );
                    value_context.params.push(format!(
                        "REGEXP {}",
                        sql_string(&text_regex(&value_param, word_start))
                    ));
                }
            }
        }
        Op::Group | Op::And | Op::Or | Op::Minus => unreachable!(),
    }
    if matches!(field, Field::Note(NoteField::Tag))
        && !matches!(op, Op::Equal)
        && let Some(last) = value_context.params.pop()
    {
        value_context.params.push(format!("t.name {}", last));
    }
    let value_str = value_context.params.join(" ");

    // Ensure field and operator have valid type
    // - Strings do not have an order
    // - Booleans do not have an order
    // - Booleans cannot use containment operator
    // - Numbers cannot use containment operator
    validate_field_operator_types(field_name, &field_type, op)
        .map_err(|e| label_error(e, comparison_span, "this comparison"))?;

    // Ensure field and value have the same type
    validate_field_value_types(field_name, &field_type, &value_type, value)
        .map_err(|e| label_error(e, value.span(), &format!("not {}", field_type.describe())))?;

    let sql_condition = if matches!(field, Field::Card(CardField::WaitingOnLinks)) {
        waiting_on_links_sql(&value_str)?
    } else {
        field.to_sql_str(&value_str)?
    };
    context.add_where_clause(sql_condition);
    Ok(())
}

/// When the card was last reviewed, or NULL if it never was. Crams and forgets are left out, as in
/// statistics. The `0` is [`ReviewLogKind::Review`].
fn last_rated_sql() -> String {
    "(SELECT MAX(rl.reviewed_at) FROM review_log rl WHERE rl.card_id = c.id AND rl.kind = 0)"
        .to_string()
}

/// How many times the card was reviewed, leaving out crams.
fn reps_sql() -> String {
    format!(
        "(SELECT COUNT(*) FROM review_log rl WHERE rl.card_id = c.id AND rl.kind = {})",
        ReviewLogKind::Review as u8
    )
}

/// How many times the card lapsed: was forgotten (rated again) in the review state, counted like
/// the scheduler's `lapses`.
fn lapses_sql() -> String {
    format!(
        "(SELECT COUNT(*) FROM review_log rl WHERE rl.card_id = c.id AND rl.kind = {} AND rl.rating = {} AND rl.previous_state = {})",
        ReviewLogKind::Review as u8,
        rating_to_number(FsrsRating::Again),
        state_to_number(FsrsState::Review),
    )
}

/// Days from the card's last review to when it is due, as in statistics, or NULL for a new card.
fn interval_sql() -> String {
    format!(
        "(CASE WHEN c.state != {} THEN (c.due - {}) / 86400.0 END)",
        state_to_number(FsrsState::New),
        last_rated_sql()
    )
}

/// Days since the card's last review over its stability, or NULL for a new card. FSRS's
/// retrievability falls as this grows, with the same curve for every card.
fn elapsed_over_stability_sql() -> String {
    format!(
        "(CASE WHEN c.state != {} THEN MAX(CAST(strftime('%s', 'now') AS INTEGER) - {}, 0) / 86400.0 / MAX(c.stability, 0.0001) END)",
        state_to_number(FsrsState::New),
        last_rated_sql()
    )
}

/// The bound on [`elapsed_over_stability_sql`] that is the same as the card's retrievability
/// comparing with `value` by `op`, under the decay of the configured FSRS parameters. SQLite has
/// no `pow`, but retrievability falls as the ratio grows, so a bound on one is a bound on the
/// other, turned around. Equal is within a millionth, as for other numbers.
fn retrievability_bound(op: Op, value: f64) -> Result<String, Error> {
    let config = read_external_config().map_err(|e| miette!("{e}"))?;
    Ok(retrievability_bound_with_decay(
        op,
        value,
        f64::from(decay(&config)),
    ))
}

fn retrievability_bound_with_decay(op: Op, value: f64, decay: f64) -> String {
    let factor = 0.9_f64.powf(-1.0 / decay) - 1.0;
    // The ratio at which retrievability is `r`: infinite for 0 or less, which no card reaches,
    // and below 0, which no card has, above 1.
    let ratio = |r: f64| {
        let ratio = (r.max(0.0).powf(-1.0 / decay) - 1.0) / factor;
        if ratio.is_infinite() {
            // SQLite reads this as infinity.
            "1e999".to_string()
        } else {
            ratio.to_string()
        }
    };
    match op {
        Op::GreaterThan => format!("< {}", ratio(value)),
        Op::GreaterThanEqual => format!("<= {}", ratio(value)),
        Op::LessThan => format!("> {}", ratio(value)),
        Op::LessThanEqual => format!(">= {}", ratio(value)),
        _ => {
            let tolerance = 1e-6 * value.abs().max(1.0);
            format!(
                "BETWEEN {} AND {}",
                ratio(value + tolerance),
                ratio(value - tolerance)
            )
        }
    }
}

/// Whether the card has a rating in a review, which compares with `value_str` if there is one,
/// at the time written in `range`, like the `>=today-7d` of `c.rated[>=today-7d]`. Crams are left
/// out, since the card's schedule is not based on them.
fn rated_sql(value_str: Option<&str>, range: Option<&str>) -> Result<String, Error> {
    let mut sql = format!(
        "EXISTS (SELECT 1 FROM review_log rl WHERE rl.card_id = c.id AND rl.kind = {}",
        ReviewLogKind::Review as u8
    );
    if let Some(value_str) = value_str {
        let _ = write!(sql, " AND rl.rating {value_str}");
    }
    if let Some(range) = range {
        let _ = write!(sql, " AND {}", review_time_sql(range)?);
    }
    sql.push(')');
    Ok(sql)
}

/// The condition on when a review was that is written in `range`, the brackets of
/// `c.rated[…]`: a comparison with a date, like `>=today-7d`, or a date, which means the review was
/// in it, like `2026-10`. Dates work as they do for other date fields.
fn review_time_sql(range: &str) -> Result<String, Error> {
    let shown = format!("c.rated[{range}]");
    let invalid = || {
        miette!(
            help = "Write a date in the brackets, or a comparison with one, as in `c.rated[today]` or `c.rated[>=today-7d]=1`",
            "`{shown}` doesn't say when the reviews were",
        )
    };
    let written = if range.starts_with(['<', '>', '=']) {
        format!("c.reviewed{range}")
    } else {
        format!("c.reviewed={range}")
    };
    let tree = Parser::new(&written)
        .parse_expression()
        .map_err(|_| invalid())?;
    let TokenTree::Cons(
        op
        @ (Op::Equal | Op::GreaterThan | Op::GreaterThanEqual | Op::LessThan | Op::LessThanEqual),
        children,
    ) = &tree
    else {
        return Err(invalid());
    };
    let [_, value] = children.as_slice() else {
        return Err(invalid());
    };
    let mut context = EvaluationContext::new(EvaluatorReturnItemType::CardIds, None);
    evaluate_comparison(
        &Field::Card(CardField::ReviewedAt),
        &shown,
        value,
        &mut context,
        *op,
        Span::default(),
    )
    // Places in `written` aren't places in the query, so the caller marks the brackets instead.
    .map_err(|e| match e.help() {
        Some(help) => miette!(
            help = help.to_string(),
            "{}",
            e.chain().last().unwrap_or(&*e)
        ),
        None => miette!("{}", e.chain().last().unwrap_or(&*e)),
    })?;
    context.where_clauses.pop().ok_or_else(invalid)
}

/// The SQL of `c.waiting_on_links` compared with `value_str`, with the `[queue]` settings of
/// `config.toml`. See [`waiting_on_links_condition`].
fn waiting_on_links_sql(value_str: &str) -> Result<String, Error> {
    let queue = read_external_config().map_err(|e| miette!("{e}"))?.queue;
    Ok(waiting_on_links_condition(&queue, value_str))
}

/// The SQL of `c.waiting_on_links` compared with `value_str`: the cards that
/// [`QueueConfig::new_after_linked_notes`] and [`QueueConfig::reviews_after_linked_notes`] hold
/// back today, so `true` matches nothing while both are off.
pub(crate) fn waiting_on_links_condition(queue: &QueueConfig, value_str: &str) -> String {
    let (_, card_due_limit) = get_start_end_local_date(&Utc::now());
    let waiting = waiting_on_links(queue, card_due_limit, None).unwrap_or_else(|| "0".to_string());
    if value_str.contains("false") {
        format!("NOT {waiting}")
    } else {
        waiting
    }
}

/// A pattern extracted from a `c.cloze ~ ...` condition for in-memory matching.
struct ClozePattern {
    /// The regex written, or the one [`text_regex`] makes for text. `None` when it is invalid.
    regex: Option<Arc<fancy_regex::Regex>>,
}

impl ClozePattern {
    fn matches(&self, text: &str) -> bool {
        self.regex
            .as_ref()
            .and_then(|re| re.is_match(text).ok())
            .unwrap_or(false)
    }
}

/// Base letters and the accented letters that `~` finds them with, so `cafe` finds `café`.
/// Upper case letters are matched by ignoring case.
const ACCENTED_LETTERS: [(char, &str); 19] = [
    ('a', "àáâãäåāăą"),
    ('c', "çćĉċč"),
    ('d', "ďđ"),
    ('e', "èéêëēĕėęě"),
    ('g', "ĝğġģ"),
    ('h', "ĥħ"),
    ('i', "ìíîïĩīĭįı"),
    ('j', "ĵ"),
    ('k', "ķ"),
    ('l', "ĺļľŀł"),
    ('n', "ñńņňŉ"),
    ('o', "òóôõöøōŏő"),
    ('r', "ŕŗř"),
    ('s', "śŝşšș"),
    ('t', "ţťŧț"),
    ('u', "ùúûüũūŭůűų"),
    ('w', "ŵ"),
    ('y', "ýÿŷ"),
    ('z', "źżž"),
];

/// A regex that finds `text` anywhere, or with `word_start` only at the start of a word, ignoring
/// case and accents in any script, so `cafe` and `CAFÉ` both find `Café`. An accent written as its
/// own combining character is ignored too. It only uses what both SQLite's `REGEXP` (the `regex`
/// crate) and `fancy_regex` support.
fn text_regex(text: &str, word_start: bool) -> String {
    let mut pattern = String::from("(?i)");
    if word_start && text.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
        pattern.push_str("\\b");
    }
    for c in text.chars() {
        let lower = c.to_lowercase().next().unwrap_or(c);
        let accented = ACCENTED_LETTERS
            .iter()
            .find(|(base, variants)| *base == lower || variants.contains(lower));
        if let Some((base, variants)) = accented {
            let _ = write!(pattern, "[{base}{variants}]\\p{{M}}*");
        } else if c.is_alphanumeric() {
            pattern.push(c);
            pattern.push_str("\\p{M}*");
        } else {
            if "\\.+*?()|[]{}^$#&-~".contains(c) {
                pattern.push('\\');
            }
            pattern.push(c);
        }
    }
    pattern
}

/// The regexes of what `query` searches the note's text for, to mark where a note matches: its
/// words, quoted text and regexes compared with `data` by `~`. Terms under `-`, and in the query of
/// a link like `linked_to_note=(verb)`, are left out, since the note need not contain them. A query
/// that can't be parsed has none.
pub(crate) fn text_patterns(query: &str) -> Vec<String> {
    let Ok(tree) = Parser::new(query).parse_expression() else {
        return Vec::new();
    };
    text_terms(&tree)
        .into_iter()
        .filter_map(|term| match term {
            Atom::Regex(r) => Some(r.to_string()),
            Atom::String(s) => Some(text_regex(s, false)),
            Atom::Word(s) => Some(text_regex(s, true)),
            _ => None,
        })
        .collect()
}

/// The words, quoted text and regexes that `tree` compares with `data` by `~`, leaving out those
/// under `-` and in the query of a link. See [`text_patterns`].
fn text_terms<'a, 'de>(tree: &'a TokenTree<'de>) -> Vec<&'a Atom<'de>> {
    fn collect<'a, 'de>(tree: &'a TokenTree<'de>, terms: &mut Vec<&'a Atom<'de>>) {
        match tree {
            TokenTree::Cons(Op::Tilde, children) => {
                if let [
                    TokenTree::Atom(Atom::Field(field), _),
                    TokenTree::Atom(value, _),
                ] = children.as_slice()
                    && matches!(Field::from_str(field), Ok(Field::Note(NoteField::Data)))
                {
                    terms.push(value);
                }
            }
            TokenTree::Cons(Op::And | Op::Or | Op::Group, children) => {
                for child in children {
                    collect(child, terms);
                }
            }
            _ => {}
        }
    }
    let mut terms = Vec::new();
    collect(tree, &mut terms);
    terms
}

/// The `ORDER BY` term that puts the notes that best match the words and quoted text `tree`
/// searches for first, by FTS5's `bm25` over the `note_fts` index, with a word in a note's keywords
/// counting double. `None` when it searches for none. Each is a phrase whose last word may start
/// a longer one, and the more of them a note has, the better it ranks. A note the index doesn't
/// match, as when a quoted text is in the middle of a word, comes after those it does.
fn relevance_order(tree: &TokenTree) -> Option<String> {
    let phrases = text_terms(tree)
        .into_iter()
        .filter_map(|term| match term {
            Atom::String(s) | Atom::Word(s) => {
                let words = s
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|word| !word.is_empty())
                    .collect::<Vec<_>>();
                (!words.is_empty()).then(|| format!("\"{}\"*", words.join(" ")))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if phrases.is_empty() {
        return None;
    }
    Some(format!(
        "IFNULL((SELECT bm25(note_fts, 1.0, 2.0) FROM note_fts WHERE note_fts MATCH {} AND note_fts.rowid = n.id), 0) ASC",
        sql_string(&phrases.join(" OR "))
    ))
}

/// The most places [`text_matches`] marks in one text.
const MAX_TEXT_MATCHES: usize = 200;

/// Where `text` matches any of `patterns`, as starts and ends in UTF-16 code units, in order, with
/// overlapping and touching matches merged. An invalid pattern matches nothing.
pub(crate) fn text_matches(patterns: &[String], text: &str) -> Vec<(usize, usize)> {
    let mut ranges = patterns
        .iter()
        .filter_map(|pattern| get_or_compile_regex(pattern).ok())
        .flat_map(|regex| {
            regex
                .find_iter(text)
                .map_while(Result::ok)
                .filter(|m| !m.range().is_empty())
                .take(MAX_TEXT_MATCHES)
                .map(|m| m.range())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged.truncate(MAX_TEXT_MATCHES);
    // Byte offsets to UTF-16 ones, counting up through the text once
    let (mut byte, mut utf16) = (0, 0);
    let mut to_utf16 = |offset: usize| {
        utf16 += text[byte..offset].encode_utf16().count();
        byte = offset;
        utf16
    };
    merged
        .into_iter()
        .map(|range| (to_utf16(range.start), to_utf16(range.end)))
        .collect()
}

/// Converts a `TokenTree` to Disjunctive Normal Form (a `Vec` of conjunctions).
///
/// Each element of the returned `Vec` is an AND-conjunction (a tree whose top-level
/// operator is `And` or a single literal). The union of all conjunctions is
/// semantically equivalent to the original tree.
///
/// This is used so that `c.cloze` post-filtering can be applied per-conjunction,
/// preserving correct AND/OR semantics across compound queries.
fn to_dnf(tree: TokenTree<'_>) -> Vec<TokenTree<'_>> {
    match tree {
        // A limited group compiles to its own subquery, so distributing into it would change
        // which items its limit keeps.
        TokenTree::Cons(Op::Group, _) if tree_has_limit(&tree) => vec![tree],
        TokenTree::Cons(Op::Or, children) => {
            // Each child is its own set of disjuncts — collect them all.
            children.into_iter().flat_map(to_dnf).collect()
        }
        TokenTree::Cons(Op::And | Op::Group, children) => {
            // Distribute AND over the disjuncts of each child (cross-product).
            let mut conjuncts: Vec<Vec<TokenTree<'_>>> = vec![vec![]];
            for child in children {
                let child_disjuncts = to_dnf(child);
                conjuncts = conjuncts
                    .into_iter()
                    .flat_map(|conj| {
                        child_disjuncts.iter().map(move |cd| {
                            let mut new_conj = conj.clone();
                            new_conj.push(cd.clone());
                            new_conj
                        })
                    })
                    .collect();
            }
            conjuncts
                .into_iter()
                .map(|mut conj| {
                    if conj.len() == 1 {
                        conj.remove(0)
                    } else {
                        TokenTree::Cons(Op::And, conj)
                    }
                })
                .collect()
        }
        // Atoms and all other Cons nodes (comparisons, Minus) are literals.
        other => vec![other],
    }
}

/// Fails if the query refers to a note or card that doesn't exist, as in `note[gir].created_at`.
/// Comparing against a missing item would otherwise match nothing, with no hint why.
async fn check_references_exist(
    query: &str,
    tree: &TokenTree<'_>,
    db: &SqlitePool,
) -> Result<(), crate::Error> {
    fn collect<'a>(tree: &'a TokenTree, dates: &mut Vec<(&'a RelativeDate, Span)>) {
        match tree {
            TokenTree::Atom(Atom::RelativeDate(date), span) => dates.push((date, *span)),
            TokenTree::Atom(_, _) => {}
            TokenTree::Cons(_, children) => {
                for child in children {
                    collect(child, dates);
                }
            }
        }
    }
    let mut dates = Vec::new();
    collect(tree, &mut dates);
    for (date, span) in dates {
        let (table, id, shown_id) = match date.anchor {
            Anchor::Note(id, _) => ("note", id.packed(), id.to_string()),
            Anchor::Card(id, _) => ("card", id, id.to_string()),
            Anchor::Today | Anchor::Now => continue,
        };
        let exists: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS (SELECT 1 FROM {table} WHERE id = ?)"
        ))
        .bind(id)
        .fetch_one(db)
        .await
        .map_err(|e| crate::Error::Sqlx { source: e })?;
        if !exists {
            return Err(search_error(query)(label_error(
                miette!("`{date}` refers to {table} `{shown_id}`, which doesn't exist"),
                span,
                &format!("no such {table}"),
            )));
        }
    }
    Ok(())
}

/// Returns true if the token tree contains a `limit` field anywhere.
pub(crate) fn tree_has_limit(tree: &TokenTree) -> bool {
    match tree {
        TokenTree::Atom(Atom::Field(f), _) => f.as_ref() == "limit",
        TokenTree::Atom(_, _) => false,
        TokenTree::Cons(_, children) => children.iter().any(tree_has_limit),
    }
}

/// Returns true if the token tree contains a `c.cloze` field reference.
fn has_cloze_field(tree: &TokenTree) -> bool {
    match tree {
        TokenTree::Atom(Atom::Field(f), _) => {
            Field::from_str(f).is_ok_and(|f| matches!(f, Field::Card(CardField::Cloze)))
        }
        TokenTree::Cons(_, children) => children.iter().any(has_cloze_field),
        TokenTree::Atom(_, _) => false,
    }
}

/// Recursively extracts all `c.cloze ~ value` patterns from the token tree.
fn extract_cloze_patterns(tree: &TokenTree) -> Vec<ClozePattern> {
    match tree {
        TokenTree::Cons(Op::Tilde, children) if children.len() == 2 => {
            let is_cloze = matches!(
                &children[0],
                TokenTree::Atom(Atom::Field(f), _)
                if Field::from_str(f).is_ok_and(|f| matches!(f, Field::Card(CardField::Cloze)))
            );
            if is_cloze {
                return match &children[1] {
                    TokenTree::Atom(Atom::Regex(r), _) => {
                        vec![ClozePattern {
                            regex: get_or_compile_regex(r).ok(),
                        }]
                    }
                    TokenTree::Atom(Atom::String(s), _) => {
                        vec![ClozePattern {
                            regex: get_or_compile_regex(&text_regex(s, false)).ok(),
                        }]
                    }
                    TokenTree::Atom(Atom::Word(s), _) => {
                        vec![ClozePattern {
                            regex: get_or_compile_regex(&text_regex(s, true)).ok(),
                        }]
                    }
                    _ => vec![],
                };
            }
            // Not a cloze field — recurse into children
            children.iter().flat_map(extract_cloze_patterns).collect()
        }
        TokenTree::Cons(_, children) => children.iter().flat_map(extract_cloze_patterns).collect(),
        TokenTree::Atom(_, _) => vec![],
    }
}

impl Evaluate for Atom<'_> {
    fn evaluate(&self, context: &mut EvaluationContext) -> Result<(), Error> {
        match self {
            Atom::Field(field) => {
                context.field = Some(Field::from_str(field)?);
                context.params.push((*field).to_string());
                Ok(())
            }
            Atom::String(s) | Atom::Word(s) => {
                context.value_type = Some(FieldType::String);
                context.params.push(s.to_string());
                Ok(())
            }
            Atom::Integer(i) => {
                context.value_type = Some(FieldType::Integer);
                context.params.push(i.to_string());
                Ok(())
            }
            Atom::Float(f) => {
                context.value_type = Some(FieldType::Float);
                context.params.push(f.to_string());
                Ok(())
            }
            Atom::Boolean(b) => {
                context.value_type = Some(FieldType::Boolean);
                context.params.push(b.to_string());
                Ok(())
            }
            Atom::DateTime(d) => {
                context.value_type = Some(FieldType::DateTime);
                context.params.push(d.timestamp().to_string());
                Ok(())
            }
            Atom::Date(d) => {
                context.value_type = Some(FieldType::DateTime);
                context.params.push(local_day_start_sql(*d, 0));
                Ok(())
            }
            Atom::LocalDateTime(d) => {
                context.value_type = Some(FieldType::DateTime);
                context.params.push(format!(
                    "(SELECT CAST(strftime('%s', '{}', 'utc') AS INTEGER))",
                    d.format("%Y-%m-%d %H:%M:%S")
                ));
                Ok(())
            }
            Atom::RelativeDate(d) => {
                context.value_type = Some(FieldType::DateTime);
                context.params.push(d.to_sql());
                Ok(())
            }
            Atom::Regex(r) => {
                context.value_type = Some(FieldType::Regex);
                context.params.push(r.to_string());
                Ok(())
            }
            Atom::Nil => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `expected` with each `text(x)` replaced by the SQL string of the regex that `~"x"` searches
    /// with, and each `text(word x)` by the one for `~x`, so expected queries stay readable.
    fn expand_text(expected: &str) -> String {
        let expected = expand_rank(expected);
        let mut out = String::new();
        let mut rest = expected.as_str();
        while let Some(start) = rest.find("text(") {
            let end = start + rest[start..].find(')').expect("`text(word ` is closed");
            out.push_str(&rest[..start]);
            let inner = &rest[start + "text(".len()..end];
            let (text, word_start) = match inner.strip_prefix("word ") {
                Some(word) => (word, true),
                None => (inner, false),
            };
            out.push_str(&sql_string(&text_regex(text, word_start)));
            rest = &rest[end + 1..];
        }
        out.push_str(rest);
        out
    }

    /// `expected` with each `rank(q)` replaced by the `ORDER BY` term that ranks notes by the FTS5
    /// query `q`.
    fn expand_rank(expected: &str) -> String {
        let mut out = String::new();
        let mut rest = expected;
        while let Some(start) = rest.find("rank(") {
            let end = start + rest[start..].find(')').expect("`rank(` is closed");
            out.push_str(&rest[..start]);
            let _ = write!(
                out,
                "IFNULL((SELECT bm25(note_fts, 1.0, 2.0) FROM note_fts WHERE note_fts MATCH {} AND note_fts.rowid = n.id), 0) ASC",
                sql_string(&rest[start + "rank(".len()..end])
            );
            rest = &rest[end + 1..];
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn explain_shows_the_tree_and_the_sql() {
        let explain = |query, output_type| Evaluator::new(query).explain(output_type).unwrap();
        assert_eq!(
            explain(
                r#"c.state=review "a (b" limit=2"#,
                EvaluatorReturnItemType::NoteIds
            ),
            expand_text(
                "Parsed:\n(and (and (= c.state review) (~  \"a (b\")) (= limit 2))\n\nSQL:\n\
                 SELECT DISTINCT n.id\nFROM note n\nLEFT JOIN card c ON n.id = c.note_id\n\
                 WHERE (c.state = 2 /* review */ AND n.data REGEXP text(a (b))\n\
                 ORDER BY rank(\"a b\"*), n.id ASC\nLIMIT 2\n"
            )
        );
        let cloze = explain("c.cloze~a or dog", EvaluatorReturnItemType::CardIds);
        assert!(
            cloze.contains("runs on its own") && cloze.matches("SELECT").count() == 2,
            "{cloze}"
        );
    }

    #[test]
    fn card_order_by_is_the_top_level_sort() {
        let order = |query| Evaluator::new(query).card_order_by().unwrap();
        assert_eq!(
            order("tag=a sort_by_desc=c.difficulty sort_by_asc=c.due"),
            ["c.difficulty DESC", "c.due ASC"]
        );
        assert_eq!(order("tag=a"), Vec::<String>::new());
        // A sort inside a limited branch only picks what the branch keeps
        assert_eq!(
            order("(tag=a sort_by_asc=c.due limit=5) or (tag=b limit=1)"),
            Vec::<String>::new()
        );
    }

    #[test]
    #[allow(clippy::too_many_lines, reason = "test data is long")]
    fn test_search_queries() {
        let inputs = vec![
            (
                "c.stability>=2.1",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE c.stability >= 2.1 ORDER BY n.id ASC",
            ),
            // Numbers are equal within a millionth.
            (
                "c.stability=2.1",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE c.stability BETWEEN 2.0999979 AND 2.1000021 ORDER BY n.id ASC",
            ),
            (
                "c.desired_retention!=0.9",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE NOT (c.desired_retention BETWEEN 0.899999 AND 0.900001) ORDER BY n.id ASC",
            ),
            // Only data search - no joins needed
            (
                "dog",
                "SELECT DISTINCT n.id FROM note n WHERE n.data REGEXP text(word dog) ORDER BY rank(\"dog\"*), n.id ASC",
            ),
            // Card count
            (
                "c.count=2",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (SELECT COUNT(*) FROM card c2 WHERE c2.note_id = n.id) = 2 ORDER BY n.id ASC",
            ),
            (
                "dog c.count>=2",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (n.data REGEXP text(word dog) AND (SELECT COUNT(*) FROM card c2 WHERE c2.note_id = n.id) >= 2) ORDER BY rank(\"dog\"*), n.id ASC",
            ),
            // Tag search - needs tag joins and special field matching syntax
            (
                "tag=math",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%')) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%'))) ORDER BY n.id ASC",
            ),
            (
                r#"tag="math:measure-theory:homework""#,
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND (t.name LIKE 'math:measure-theory:homework' OR t.name LIKE 'math:measure-theory:homework:%')) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND (t.name LIKE 'math:measure-theory:homework' OR t.name LIKE 'math:measure-theory:homework:%'))) ORDER BY n.id ASC",
            ),
            // Exclude tag
            (
                "-tag=math",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE NOT ((EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%')) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%')))) ORDER BY n.id ASC",
            ),
            // linked_to_note
            (
                "linked_to_note=gir",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN note_link nl ON n.id = nl.parent_note_id WHERE EXISTS (SELECT 1 FROM note_link nl WHERE nl.parent_note_id = n.id AND nl.linked_note_id = 76967537148) ORDER BY n.id ASC",
            ),
            // Exclude linked_to_note
            (
                "-linked_to_note=gir",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN note_link nl ON n.id = nl.parent_note_id WHERE NOT (EXISTS (SELECT 1 FROM note_link nl WHERE nl.parent_note_id = n.id AND nl.linked_note_id = 76967537148)) ORDER BY n.id ASC",
            ),
            // linked_to_keyword
            (
                r#"linked_to_keyword="foo""#,
                "SELECT DISTINCT n.id FROM note n LEFT JOIN note_link nl ON n.id = nl.parent_note_id WHERE EXISTS (SELECT 1 FROM note_link nl WHERE nl.parent_note_id = n.id AND nl.searched_keyword = 'foo') ORDER BY n.id ASC",
            ),
            // linked_from_note
            (
                "linked_from_note=a",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN note_link nl ON n.id = nl.parent_note_id WHERE EXISTS (SELECT 1 FROM note_link nl WHERE nl.linked_note_id = n.id AND nl.parent_note_id = 10460353203) ORDER BY n.id ASC",
            ),
            // Note ids, in any case, compared in the order of their letters
            (
                "id=GIR",
                "SELECT DISTINCT n.id FROM note n WHERE n.id = 76967537148 ORDER BY n.id ASC",
            ),
            (
                "id>=gir and id<\"gja\"",
                "SELECT DISTINCT n.id FROM note n WHERE (n.id >= 76967537148 AND n.id < 77111026218) ORDER BY n.id ASC",
            ),
            // Convert types
            (
                "tag=2 or tag=true",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE ((EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND (t.name LIKE '2' OR t.name LIKE '2:%')) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND (t.name LIKE '2' OR t.name LIKE '2:%'))) OR (EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND (t.name LIKE 'true' OR t.name LIKE 'true:%')) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND (t.name LIKE 'true' OR t.name LIKE 'true:%')))) ORDER BY n.id ASC",
            ),
            // Card search - needs card join
            (
                "-card.scheduler_buried=true",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE NOT (c.special_state = 3) ORDER BY n.id ASC",
            ),
            // Complex query - needs multiple joins
            (
                "dog and tag=math and -card.suspended=true",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE ((n.data REGEXP text(word dog) AND (EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%')) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%')))) AND NOT (c.special_state = 1)) ORDER BY rank(\"dog\"*), n.id ASC",
            ),
            (
                "dog and tag=math and card.suspended=false",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE ((n.data REGEXP text(word dog) AND (EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%')) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%')))) AND (c.special_state IS NULL OR c.special_state != 1)) ORDER BY rank(\"dog\"*), n.id ASC",
            ),
            // Custom data - no joins needed
            (
                "custom_data.x.y[1]>=123",
                "SELECT DISTINCT n.id FROM note n WHERE COALESCE(json_extract(n.custom_data, '$.x.y[1]') >= 123, 0) ORDER BY n.id ASC",
            ),
            // Custom data with string value containing special characters
            (
                r#"custom_data.card_key="preflop-hand|SB|K5o|default""#,
                "SELECT DISTINCT n.id FROM note n WHERE COALESCE(json_extract(n.custom_data, '$.card_key') = 'preflop-hand|SB|K5o|default', 0) ORDER BY n.id ASC",
            ),
            // A path on its own: the key is there and isn't `false` or `null`
            (
                "custom_data.flag -card.custom_data.a.b[0]",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (COALESCE(json_type(n.custom_data, '$.flag') NOT IN ('false', 'null'), 0) AND NOT (COALESCE(json_type(c.custom_data, '$.a.b[0]') NOT IN ('false', 'null'), 0))) ORDER BY n.id ASC",
            ),
            (
                "-custom_data.a.b=true c.custom_data.array[0].key>=1",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (NOT (COALESCE(json_extract(n.custom_data, '$.a.b') = true, 0)) AND COALESCE(json_extract(c.custom_data, '$.array[0].key') >= 1, 0)) ORDER BY n.id ASC",
            ),
            // Regex search
            (
                "re:\"\\d{3}\"",
                "SELECT DISTINCT n.id FROM note n WHERE n.data REGEXP '\\d{3}' ORDER BY n.id ASC",
            ),
            (
                "data~re:\"\\d{3}\"",
                "SELECT DISTINCT n.id FROM note n WHERE n.data REGEXP '\\d{3}' ORDER BY n.id ASC",
            ),
            (
                "tag~re:\"^math\"",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND t.name REGEXP '^math') OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND t.name REGEXP '^math')) ORDER BY n.id ASC",
            ),
            // Negated groups
            (
                "-(dog or cat)",
                "SELECT DISTINCT n.id FROM note n WHERE NOT ((n.data REGEXP text(word dog) OR n.data REGEXP text(word cat))) ORDER BY n.id ASC",
            ),
            (
                "-(tag=math or dog)",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE NOT (((EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%')) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%'))) OR n.data REGEXP text(word dog))) ORDER BY n.id ASC",
            ),
            (
                "-(tag=math and data~foo)",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE NOT (((EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%')) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%'))) AND n.data REGEXP text(word foo))) ORDER BY n.id ASC",
            ),
            (
                "tag=math and -(data~re:\"20[0-9][0-9]-(fall|winter)-[0-9]+\" or data~re:\"fall[0-9][0-9]-sol[0-9]-[0-9]+\")",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE ((EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%')) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%'))) AND NOT ((n.data REGEXP '20[0-9][0-9]-(fall|winter)-[0-9]+' OR n.data REGEXP 'fall[0-9][0-9]-sol[0-9]-[0-9]+'))) ORDER BY n.id ASC",
            ),
            // Values are quoted and LIKE wildcards are matched literally.
            (
                r#""don't" snake_case "100%" re:"it's""#,
                r#"SELECT DISTINCT n.id FROM note n WHERE (((n.data REGEXP text(don't) AND n.data REGEXP text(word snake_case)) AND n.data REGEXP text(100%)) AND n.data REGEXP 'it''s') ORDER BY rank("don t"* OR "snake case"* OR "100"*), n.id ASC"#,
            ),
            (
                "parser_name=name",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN parser p ON n.parser_id = p.id WHERE p.name = 'name' ORDER BY n.id ASC",
            ),
            (
                r#"custom_data["a's key"]>=1 tag="a_b""#,
                r#"SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (COALESCE(json_extract(n.custom_data, '$."a''s key"') >= 1, 0) AND (EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND (t.name LIKE 'a\_b' ESCAPE '\' OR t.name LIKE 'a\_b:%' ESCAPE '\')) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND (t.name LIKE 'a\_b' ESCAPE '\' OR t.name LIKE 'a\_b:%' ESCAPE '\')))) ORDER BY n.id ASC"#,
            ),
            // Words that start with a keyword, numbers and bare boolean fields
            (
                "orbit 2024 c.user_buried",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE ((n.data REGEXP text(word orbit) AND n.data REGEXP text(word 2024)) AND c.special_state = 2) ORDER BY rank(\"orbit\"* OR \"2024\"*), n.id ASC",
            ),
            // Card states by name
            (
                "c.state=new or c.state!=Review",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (c.state = 0 /* new */ OR NOT (c.state = 2 /* review */)) ORDER BY n.id ASC",
            ),
            // `!=` and `not` negate
            (
                "c.state!=0 not dog",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (NOT (c.state = 0 /* new */) AND NOT (n.data REGEXP text(word dog))) ORDER BY n.id ASC",
            ),
            // `and` binds tighter than `or`
            (
                "a or b c",
                "SELECT DISTINCT n.id FROM note n WHERE (n.data REGEXP text(word a) OR (n.data REGEXP text(word b) AND n.data REGEXP text(word c))) ORDER BY rank(\"a\"* OR \"b\"* OR \"c\"*), n.id ASC",
            ),
        ];
        for (input, expected_sql) in inputs {
            let evaluator = Evaluator::new(input);
            let query_str = evaluator
                .evaluate(EvaluatorReturnItemType::NoteIds)
                .unwrap();
            assert_eq!(
                query_str,
                expand_text(expected_sql),
                "Failed for input: {}",
                input
            );
        }
    }

    #[test]
    fn field_names_are_fields() {
        for field in FIELDS {
            assert!(Field::from_str(field.name).is_ok(), "{}", field.name);
        }
    }

    #[test]
    fn type_errors_say_how_to_write_the_value() {
        let error = |query| {
            render_error(
                query,
                &Evaluator::new(query)
                    .evaluate(EvaluatorReturnItemType::NoteIds)
                    .unwrap_err(),
            )
        };
        assert_eq!(
            error("c.due=tomorrow"),
            "`c.due` holds a date, so it can't be compared with text `tomorrow`\n\nhelp: Write a date, like `2026-10-17`, `today+1d` or `now-12h`, as in `c.due<=today`"
        );
        assert_eq!(
            error("card.suspended=yes"),
            "`card.suspended` holds `true` or `false`, so it can't be compared with text `yes`\n\nhelp: Write `true` or `false`, as in `card.suspended=false`"
        );
        assert_eq!(
            error("c.stability~2"),
            "`c.stability` holds a number, so it can't use `~`\n\nhelp: Use `=`, `<`, `<=`, `>` or `>=`, as in `c.stability>=30`"
        );
    }

    #[test]
    fn unknown_fields_suggest_similar_ones() {
        assert_eq!(similar_fields("tags"), ["tag"]);
        assert_eq!(similar_fields("due"), ["c.due"]);
        assert_eq!(similar_fields("sort_by"), ["sort_by_asc", "sort_by_desc"]);
        assert_eq!(similar_fields("c.stabilty"), ["c.stability"]);
        assert_eq!(similar_fields("Keywords"), ["keyword"]);
        assert_eq!(similar_fields("zebra"), Vec::<&str>::new());
        let error = Field::from_str("sort_by").unwrap_err().to_string();
        assert_eq!(
            error,
            "Unknown field `sort_by`: did you mean `sort_by_asc` or `sort_by_desc`?"
        );
    }

    #[test]
    fn test_search_query_errors() {
        let inputs = [
            // Missing value
            "tag=",
            // Invalid value
            "tag:personal",
            "-tag:personal",
            // Invalid field
            "tags=math",
            // Field type and value type do not match
            "card.scheduler_buried=test",
            // Field type and operator type do not match
            "data>2", // Strings do not have an order
            "id~2",   // Numbers cannot use containment operator
            // Note ids are letters, so old numeric ids are rejected rather than reinterpreted.
            "id>100",
            "id=42",
            "linked_to_note=12",
            "linked_from_note=5",
            "id=gi-r",
            "id=abcdefghi",
            "c.suspended~2", // Booleans cannot use containment operator
            "c.suspended>2", // Booleans do not have an order
            // String as field is not allowed
            "\"test\"=2",
            // The old form of custom data paths
            "custom_data:\"$.x.y[1]\"=1",
            // Malformed custom data paths
            "custom_data.x.=1",
            "custom_data..x=1",
            "custom_data[x]=1",
            "custom_data[]",
            r#"custom_data["a\"b"]=1"#,
            "custom_data.x and",
            // Dangling operator
            "tag=math and",
            "or tag=math",
            // Unbalanced parentheses
            "(dog",
            "dog ) cat",
            "()",
            // Invalid regex usage
            "data=re:\"math\"",
            // Not a card state
            "c.state=suspended",
            // Not a time of reviews
            "c.rated[]=1",
            "c.rated[>=]=1",
            "c.rated[soon]=1",
            "c.rated[~x]=1",
            "c.rated[today",
            "c.rated[today]~1",
            "sort_by_asc=c.rated[today]",
            "c.retrievability~0.5",
            // Queries in parentheses only go with link fields, and only say which notes
            "tag=(math)",
            "linked_to_note~(tag=math)",
            "linked_to_note=()",
            "linked_to_note=(tag=math limit=1)",
            "linked_to_note=(tag=math sort_by_asc=id)",
            "linked_from_note=(c.cloze~x)",
            "linked_to_note=(tag=math",
        ];
        for input in inputs {
            let evaluator = Evaluator::new(input);
            let query_str_res = evaluator.evaluate(EvaluatorReturnItemType::NoteIds);
            assert!(
                query_str_res.is_err(),
                "Expected error for input: {}",
                input
            );
        }
    }

    #[test]
    #[allow(clippy::too_many_lines, reason = "test data is long")]
    fn test_relative_dates() {
        let gir = "(SELECT created_at FROM note WHERE id = 76967537148)";
        let today =
            "(SELECT CAST(strftime('%s', 'now', 'localtime', 'start of day', 'utc') AS INTEGER))";
        let inputs = [
            (
                "created_at<=note[gir].created_at",
                format!("SELECT DISTINCT n.id FROM note n WHERE n.created_at <= {gir} ORDER BY n.id ASC"),
            ),
            (
                "created_at<=note[gir].created_at and created_at>=note[gir].created_at-5d",
                format!(
                    "SELECT DISTINCT n.id FROM note n WHERE (n.created_at <= {gir} AND n.created_at >= (SELECT CAST(strftime('%s', {gir}, 'unixepoch', 'localtime', '-5 days', 'utc') AS INTEGER))) ORDER BY n.id ASC"
                ),
            ),
            (
                "updated_at>note[gir].updated_at",
                "SELECT DISTINCT n.id FROM note n WHERE n.updated_at > (SELECT updated_at FROM note WHERE id = 76967537148) ORDER BY n.id ASC".to_string(),
            ),
            (
                "c.due<=card[12].due",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE c.due <= (SELECT due FROM card WHERE id = 12) ORDER BY n.id ASC".to_string(),
            ),
            (
                "created_at>=today",
                format!("SELECT DISTINCT n.id FROM note n WHERE n.created_at >= {today} ORDER BY n.id ASC"),
            ),
            // A day stands for the whole day, so this is up to the end of the day 3 days from now.
            (
                "c.due<=today+3d",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE c.due < (SELECT CAST(strftime('%s', 'now', 'localtime', 'start of day', '+3 days', '+1 days', 'utc') AS INTEGER)) ORDER BY n.id ASC".to_string(),
            ),
            (
                "created_at=today",
                format!("SELECT DISTINCT n.id FROM note n WHERE n.created_at BETWEEN {today} AND ((SELECT CAST(strftime('%s', 'now', 'localtime', 'start of day', '+1 days', 'utc') AS INTEGER)) - 1) ORDER BY n.id ASC"),
            ),
            (
                "created_at>today-1w",
                "SELECT DISTINCT n.id FROM note n WHERE n.created_at >= (SELECT CAST(strftime('%s', 'now', 'localtime', 'start of day', '-7 days', '+1 days', 'utc') AS INTEGER)) ORDER BY n.id ASC".to_string(),
            ),
            // A fixed date is a local day too.
            (
                "created_at=2026-10-17",
                "SELECT DISTINCT n.id FROM note n WHERE n.created_at BETWEEN (SELECT CAST(strftime('%s', '2026-10-17', '+0 days', 'utc') AS INTEGER)) AND ((SELECT CAST(strftime('%s', '2026-10-17', '+1 days', 'utc') AS INTEGER)) - 1) ORDER BY n.id ASC".to_string(),
            ),
            (
                "created_at<2026-10-17 c.due<=2026-10-17",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (n.created_at < (SELECT CAST(strftime('%s', '2026-10-17', '+0 days', 'utc') AS INTEGER)) AND c.due < (SELECT CAST(strftime('%s', '2026-10-17', '+1 days', 'utc') AS INTEGER))) ORDER BY n.id ASC".to_string(),
            ),
            // So are a month and a year.
            (
                "created_at=2026-10 c.due<2026",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (n.created_at BETWEEN (SELECT CAST(strftime('%s', '2026-10-01', '+0 months', 'utc') AS INTEGER)) AND ((SELECT CAST(strftime('%s', '2026-10-01', '+1 months', 'utc') AS INTEGER)) - 1) AND c.due < (SELECT CAST(strftime('%s', '2026-01-01', '+0 months', 'utc') AS INTEGER))) ORDER BY n.id ASC".to_string(),
            ),
            (
                "created_at>2026",
                "SELECT DISTINCT n.id FROM note n WHERE n.created_at >= (SELECT CAST(strftime('%s', '2026-01-01', '+12 months', 'utc') AS INTEGER)) ORDER BY n.id ASC".to_string(),
            ),
            // A time without a zone is local.
            (
                "created_at>=2026-10-17T14:30 created_at<2026-10-17T14:30:05",
                "SELECT DISTINCT n.id FROM note n WHERE (n.created_at >= (SELECT CAST(strftime('%s', '2026-10-17 14:30:00', 'utc') AS INTEGER)) AND n.created_at < (SELECT CAST(strftime('%s', '2026-10-17 14:30:05', 'utc') AS INTEGER))) ORDER BY n.id ASC".to_string(),
            ),
            // Against text, they are text.
            (
                "keyword=2026-10 or keyword=2026",
                "SELECT DISTINCT n.id FROM note n WHERE (EXISTS (SELECT 1 FROM note_keyword nk WHERE nk.note_id = n.id AND nk.keyword = '2026-10') OR EXISTS (SELECT 1 FROM note_keyword nk WHERE nk.note_id = n.id AND nk.keyword = '2026')) ORDER BY n.id ASC".to_string(),
            ),
            // A time is exact.
            (
                "created_at>=2026-10-17T12:00:00Z",
                "SELECT DISTINCT n.id FROM note n WHERE n.created_at >= 1792238400 ORDER BY n.id ASC".to_string(),
            ),
            // Against text, a date is text.
            (
                "data~2026-10-17",
                "SELECT DISTINCT n.id FROM note n WHERE n.data REGEXP text(word 2026-10-17) ORDER BY n.id ASC".to_string(),
            ),
            (
                "-created_at>=now-12h",
                "SELECT DISTINCT n.id FROM note n WHERE NOT (n.created_at >= (SELECT CAST(strftime('%s', 'now', '-12 hours') AS INTEGER))) ORDER BY n.id ASC".to_string(),
            ),
            // `today` and `now` are still words when the field isn't a date.
            (
                "today",
                "SELECT DISTINCT n.id FROM note n WHERE n.data REGEXP text(word today) ORDER BY rank(\"today\"*), n.id ASC".to_string(),
            ),
            (
                "data~now",
                "SELECT DISTINCT n.id FROM note n WHERE n.data REGEXP text(word now) ORDER BY rank(\"now\"*), n.id ASC".to_string(),
            ),
            (
                "keyword=today-5d",
                "SELECT DISTINCT n.id FROM note n WHERE EXISTS (SELECT 1 FROM note_keyword nk WHERE nk.note_id = n.id AND nk.keyword = 'today-5d') ORDER BY n.id ASC".to_string(),
            ),
            // `note` followed by a group is not a reference.
            (
                "note(dog or cat)",
                "SELECT DISTINCT n.id FROM note n WHERE (n.data REGEXP text(word note) AND (n.data REGEXP text(word dog) OR n.data REGEXP text(word cat))) ORDER BY rank(\"note\"* OR \"dog\"* OR \"cat\"*), n.id ASC".to_string(),
            ),
        ];
        for (input, expected_sql) in inputs {
            let query_str = Evaluator::new(input)
                .evaluate(EvaluatorReturnItemType::NoteIds)
                .unwrap();
            assert_eq!(
                query_str,
                expand_text(&expected_sql),
                "Failed for input: {}",
                input
            );
        }
    }

    #[test]
    fn test_relative_date_errors() {
        let inputs = [
            // Not a date field
            "c.stability>=note[gir].created_at",
            "id=note[gir].created_at",
            // A date can't be searched for as text
            "created_at~note[gir].created_at",
            "created_at~today",
            "created_at~2026-10-17",
            "created_at~2026-10",
            "created_at=2026-13",
            "created_at=12345",
            "created_at=2026-10-17T25:00",
            "created_at=2026-10-17T14",
            // Malformed references and offsets
            "created_at>=note[gir",
            "created_at>=note[gir].due",
            "created_at>=note[12].created_at",
            "created_at>=card[gir].due",
            "created_at>=today-5",
            "created_at>=today-5x",
            "created_at>=nowish",
        ];
        for input in inputs {
            let query_str_res = Evaluator::new(input).evaluate(EvaluatorReturnItemType::NoteIds);
            assert!(
                query_str_res.is_err(),
                "Expected error for input: {}",
                input
            );
        }
    }

    #[sqlx::test]
    async fn relative_dates_select_by_another_notes_timestamp(pool: SqlitePool) {
        let now = chrono::Utc::now().timestamp();
        let day = 24 * 60 * 60;
        sqlx::query("INSERT INTO parser (id, name) VALUES (1, 'markdown')")
            .execute(&pool)
            .await
            .unwrap();
        // `d` is the reference note. `b` and `c` were created in the 5 days before it, `a` before
        // that and `e` after it.
        let notes = [
            ("a", now - 20 * day),
            ("b", now - 14 * day),
            ("c", now - 10 * day - 60),
            ("d", now - 10 * day),
            ("e", now - 60),
        ];
        for (letters, created_at) in notes {
            sqlx::query(
                "INSERT INTO note (id, data, created_at, updated_at, custom_data, parser_id) VALUES (?, '', ?, ?, '{}', 1)",
            )
            .bind(letters.parse::<NoteId>().unwrap())
            .bind(created_at)
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap();
        }
        let search = async |query: &str| {
            let mut ids = Evaluator::new(query)
                .get_note_ids(&pool)
                .await?
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>();
            ids.sort();
            Ok::<_, crate::Error>(ids)
        };

        assert_eq!(
            search("created_at<=note[d].created_at and created_at>=note[d].created_at-5d")
                .await
                .unwrap(),
            ["b", "c", "d"]
        );
        assert_eq!(
            search("created_at>note[d].created_at").await.unwrap(),
            ["e"]
        );
        // `e` was created a minute ago, so it is after the start of yesterday at any time of day.
        assert_eq!(search("created_at>=today-1d").await.unwrap(), ["e"]);
        assert_eq!(search("created_at<now-15d").await.unwrap(), ["a"]);

        let error = search("created_at<=note[z].created_at").await.unwrap_err();
        assert!(
            error.to_string().contains("note `z`, which doesn't exist"),
            "{error}"
        );
    }

    #[sqlx::test]
    async fn searches_text_tags_and_days(
        pool_options: sqlx::sqlite::SqlitePoolOptions,
        options: sqlx::sqlite::SqliteConnectOptions,
    ) {
        // Text searches need `REGEXP`.
        let pool = crate::helpers::pool_with_regexp(pool_options, options).await;
        let exec = async |sql: &str| {
            sqlx::query(sql).execute(&pool).await.unwrap();
        };
        exec("INSERT INTO parser (id, name) VALUES (1, 'markdown')").await;
        let yesterday = chrono::Local::now().date_naive() - chrono::Days::new(1);
        let start_of_yesterday = yesterday
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .earliest()
            .unwrap()
            .timestamp();
        // `a` to `c` have cards, `d` has none. `b` was created at the start of yesterday, `c` at
        // the end of it, and the others today.
        let now = chrono::Utc::now().timestamp();
        let notes = [
            ("a", "don't use snake_case", now),
            ("b", "dont use snakeXcase", start_of_yesterday),
            ("c", "100% orbit", start_of_yesterday + 24 * 60 * 60 - 1),
            ("d", "1000 or bit", now),
        ];
        for (letters, data, created_at) in notes {
            sqlx::query(
                "INSERT INTO note (id, data, created_at, updated_at, custom_data, parser_id) VALUES (?, ?, ?, ?, '{}', 1)",
            )
            .bind(letters.parse::<NoteId>().unwrap())
            .bind(data)
            .bind(created_at)
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap();
        }
        for letters in ["a", "b", "c"] {
            sqlx::query(
                r#"INSERT INTO card (note_id, "order", back_type, due, stability, difficulty,
                   desired_retention, state, custom_data) VALUES (?, 1, 1, 0, 0, 0, 0.9, 0, '{}')"#,
            )
            .bind(letters.parse::<NoteId>().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        }
        exec("INSERT INTO tag (id, name, description, auto_delete) VALUES (1, 'math', '', 0), (2, 'math:algebra', '', 0), (3, 'physics:math', '', 0), (4, 'mathx', '', 0)").await;
        // `a` is tagged `math`, `b` a descendant, `c` has `math` in the middle and `d` (with no
        // cards) is `mathx`.
        exec(&format!(
            "INSERT INTO note_tag (note_id, tag_id) VALUES ({}, 1), ({}, 2), ({}, 3), ({}, 4)",
            "a".parse::<NoteId>().unwrap().packed(),
            "b".parse::<NoteId>().unwrap().packed(),
            "c".parse::<NoteId>().unwrap().packed(),
            "d".parse::<NoteId>().unwrap().packed(),
        ))
        .await;

        let search = async |query: &str| {
            let mut ids = Evaluator::new(query)
                .get_note_ids(&pool)
                .await
                .unwrap()
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>();
            ids.sort();
            ids
        };
        assert_eq!(search("\"don't\"").await, ["a"]);
        assert_eq!(search("snake_case").await, ["a"]);
        assert_eq!(search("\"100%\"").await, ["c"]);
        assert_eq!(search("orbit").await, ["c"]);
        assert_eq!(search("1000").await, ["d"]);
        // A word finds words that start with it, and quoted text finds it anywhere.
        assert_eq!(search("bit").await, ["d"]);
        assert_eq!(search("\"bit\"").await, ["c", "d"]);
        assert_eq!(search("snake").await, ["a", "b"]);
        assert_eq!(search("case").await, Vec::<String>::new());
        assert_eq!(search("data~\"case\"").await, ["a", "b"]);
        // Case and accents are ignored, in the query and in the note.
        exec(&format!(
            "UPDATE note SET data = 'Ça Fait Été, ﬁn' WHERE id = {}",
            "d".parse::<NoteId>().unwrap().packed()
        ))
        .await;
        assert_eq!(search("\"CA FAIT\"").await, ["d"]);
        assert_eq!(search("\"fait ete\"").await, ["d"]);
        assert_eq!(search("ÉTÉ").await, ["d"]);
        exec(&format!(
            "UPDATE note SET data = 'cafe\u{301} Straße ΣΊΣΥΦΟΣ' WHERE id = {}",
            "d".parse::<NoteId>().unwrap().packed()
        ))
        .await;
        assert_eq!(search("café").await, ["d"]);
        assert_eq!(search("straße σίσυφος").await, ["d"]);
        assert_eq!(search("tag=math").await, ["a", "b"]);
        assert_eq!(search("tag=mathx").await, ["d"]);
        assert_eq!(search("-tag=math").await, ["c", "d"]);
        assert_eq!(search("created_at=today-1d").await, ["b", "c"]);
        assert_eq!(search(&format!("created_at={yesterday}")).await, ["b", "c"]);
        assert_eq!(search(&format!("created_at>{yesterday}")).await, ["a", "d"]);
        assert_eq!(search("created_at<today").await, ["b", "c"]);
        // `0.9` as an `f32` is still `0.9`.
        exec(&format!(
            "UPDATE card SET desired_retention = {} WHERE note_id = {}",
            f64::from(0.9_f32),
            "a".parse::<NoteId>().unwrap().packed()
        ))
        .await;
        assert_eq!(search("c.desired_retention=0.9").await, ["a", "b", "c"]);
        // `a` and `d` were created this month and year, and `b` and `c` too unless yesterday was in
        // the last one.
        let today = chrono::Local::now().date_naive();
        let this_month = search(&format!("created_at={}", today.format("%Y-%m"))).await;
        assert!(
            this_month.starts_with(&["a".to_string()]) && this_month.contains(&"d".to_string())
        );
        let this_year = search(&format!("created_at>={}", today.format("%Y"))).await;
        assert!(this_year.contains(&"a".to_string()) && this_year.contains(&"d".to_string()));
        assert_eq!(
            search(&format!("created_at>{}", today.format("%Y"))).await,
            Vec::<String>::new()
        );
    }

    #[sqlx::test]
    #[allow(clippy::too_many_lines, reason = "test data is long")]
    async fn searches_review_history(pool: SqlitePool) {
        let exec = async |sql: &str| {
            sqlx::query(sql).execute(&pool).await.unwrap();
        };
        let now = chrono::Utc::now().timestamp();
        let day = 24 * 60 * 60;
        exec("INSERT INTO parser (id, name) VALUES (1, 'markdown')").await;
        // `a` was reviewed 20, 10 and 2 days ago, lapsing 10 days ago, and is due in 5 days. `b`
        // was reviewed 30 days ago and crammed yesterday. `c` is new.
        let cards = [
            ("a", 1, 2, 10.0, now + 5 * day),
            ("b", 2, 2, 1.0, now + day),
            ("c", 3, 0, 0.0, now),
        ];
        for (letters, card_id, state, stability, due) in cards {
            let note_id = letters.parse::<NoteId>().unwrap();
            sqlx::query(
                "INSERT INTO note (id, data, created_at, updated_at, custom_data, parser_id) VALUES (?, '', 0, 0, '{}', 1)",
            )
            .bind(note_id)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                r#"INSERT INTO card (id, note_id, "order", back_type, due, stability, difficulty,
                   desired_retention, state, custom_data) VALUES (?, ?, 1, 1, ?, ?, 5, 0.9, ?, '{}')"#,
            )
            .bind(card_id)
            .bind(note_id)
            .bind(due)
            .bind(stability)
            .bind(state)
            .execute(&pool)
            .await
            .unwrap();
        }
        // (card, days ago, kind, rating, previous state)
        let reviews = [
            (1, 20, 0, 3, 0),
            (1, 10, 0, 1, 2),
            (1, 2, 0, 3, 3),
            (2, 30, 0, 4, 0),
            (2, 1, 16, 1, 2),
        ];
        for (card_id, days_ago, kind, rating, previous_state) in reviews {
            sqlx::query(
                "INSERT INTO review_log (card_id, reviewed_at, kind, rating, scheduler_name, previous_state, custom_data) VALUES (?, ?, ?, ?, 'fsrs', ?, '{}')",
            )
            .bind(card_id)
            .bind(now - days_ago * day)
            .bind(kind)
            .bind(rating)
            .bind(previous_state)
            .execute(&pool)
            .await
            .unwrap();
        }
        let search = async |query: &str| {
            Evaluator::new(query)
                .get_card_ids(&pool)
                .await
                .unwrap_or_else(|e| panic!("{query}: {e}"))
        };
        assert_eq!(search("c.reviewed>=today-7d").await, [1]);
        assert_eq!(search("-c.reviewed>=today-7d").await, [3, 2]);
        assert_eq!(search("c.rated[>=today-7d]").await, [1]);
        assert_eq!(search("c.rated[>=today-15d]=1").await, [1]);
        assert_eq!(search("c.rated[>=today-5d]=1").await, Vec::<CardId>::new());
        // The cram is not a rating.
        assert_eq!(search("c.rated=1").await, [1]);
        assert_eq!(search("card.rated[<today-25d]=4").await, [2]);
        assert_eq!(search("c.rated[today-2d]").await, [1]);
        assert_eq!(search("c.reps>=2").await, [1]);
        assert_eq!(search("c.reps=0").await, [3]);
        assert_eq!(search("c.lapses=1").await, [1]);
        assert_eq!(search("c.interval=7").await, [1]);
        assert_eq!(
            search("c.interval>=1 c.interval<2").await,
            Vec::<CardId>::new()
        );
        assert_eq!(search("c.retrievability<0.8").await, [2]);
        assert_eq!(search("-c.retrievability<0.8").await, [3, 1]);
        assert_eq!(search("c.retrievability>0").await, [2, 1]);
        assert_eq!(search("c.retrievability>1").await, Vec::<CardId>::new());
        // The same retrievability as FSRS gives
        let config = read_external_config().unwrap();
        let recall = crate::schedulers::fsrs::utils::retrievability(2.0, 10.0, decay(&config));
        assert_eq!(
            search(&format!(
                "c.retrievability>{} c.retrievability<{}",
                recall - 0.001,
                recall + 0.001
            ))
            .await,
            [1]
        );
        // Cards with no retrievability come last.
        assert_eq!(search("sort_by_asc=c.retrievability").await, [2, 1, 3]);
        assert_eq!(search("sort_by_desc=c.retrievability").await, [1, 2, 3]);
        assert_eq!(search("sort_by_desc=c.lapses,c.id").await, [1, 3, 2]);
    }

    #[sqlx::test]
    async fn searches_notes_linked_to_a_query(pool: SqlitePool) {
        let exec = async |sql: &str| {
            sqlx::query(sql).execute(&pool).await.unwrap();
        };
        let id = |letters: &str| letters.parse::<NoteId>().unwrap().packed();
        exec("INSERT INTO parser (id, name) VALUES (1, 'markdown')").await;
        exec("INSERT INTO tag (id, name, description, auto_delete) VALUES (1, 'math', '', 0)")
            .await;
        // `a` links to `m`, which is tagged `math`, `b` links to `a`, `c` links to `x`, which isn't,
        // and `d` links to nothing.
        for letters in ["a", "b", "c", "d", "m", "x"] {
            exec(&format!(
                "INSERT INTO note (id, data, created_at, updated_at, custom_data, parser_id) VALUES ({}, '', 0, 0, '{{}}', 1)",
                id(letters)
            ))
            .await;
        }
        exec(&format!(
            "INSERT INTO note_tag (note_id, tag_id) VALUES ({}, 1)",
            id("m")
        ))
        .await;
        for (order, (from, to)) in [("a", "m"), ("b", "a"), ("c", "x")].into_iter().enumerate() {
            exec(&format!(
                "INSERT INTO note_link (parent_note_id, linked_note_id, \"order\", searched_keyword) VALUES ({}, {}, {order}, 'k')",
                id(from),
                id(to)
            ))
            .await;
        }
        let search = async |query: &str| {
            Evaluator::new(query)
                .get_note_ids(&pool)
                .await
                .unwrap_or_else(|e| panic!("{query}: {e}"))
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(search("linked_to_note=(tag=math)").await, ["a"]);
        assert_eq!(
            search("linked_from_note=(tag=math)").await,
            Vec::<String>::new()
        );
        assert_eq!(search("linked_from_note=(linked_to_note=m)").await, ["m"]);
        assert_eq!(
            search("-linked_to_note=(tag=math)").await,
            ["b", "c", "d", "m", "x"]
        );
        assert_eq!(
            search("linked_to_note!=(tag=math)").await,
            search("-linked_to_note=(tag=math)").await
        );
        assert_eq!(
            search("linked_to_note=(linked_to_note=(tag=math))").await,
            ["b"]
        );
        assert_eq!(search("linked_to_note=(-tag=math)").await, ["b", "c"]);
        // Still a single note without parentheses
        assert_eq!(search("linked_to_note=x").await, ["c"]);
    }

    #[sqlx::test]
    async fn ranks_notes_found_by_words(
        pool_options: sqlx::sqlite::SqlitePoolOptions,
        options: sqlx::sqlite::SqliteConnectOptions,
    ) {
        let pool = crate::helpers::pool_with_regexp(pool_options, options).await;
        let exec = async |sql: &str| {
            sqlx::query(sql).execute(&pool).await.unwrap();
        };
        let id = |letters: &str| letters.parse::<NoteId>().unwrap().packed();
        exec("INSERT INTO parser (id, name) VALUES (1, 'markdown')").await;
        let notes = [
            ("a", "a dog sat on a mat"),
            ("b", "the cat and the dog"),
            ("c", "a cat sat on a mat"),
            ("d", "nothing here"),
        ];
        for (letters, data) in notes {
            exec(&format!(
                "INSERT INTO note (id, data, created_at, updated_at, custom_data, parser_id) VALUES ({}, '{data}', 0, 0, '{{}}', 1)",
                id(letters)
            ))
            .await;
        }
        let search = async |query: &str| {
            Evaluator::new(query)
                .get_note_ids(&pool)
                .await
                .unwrap()
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
        };
        // The note with both words first
        assert_eq!(search("dog or cat").await, ["b", "a", "c"]);
        // A sort takes the place of relevance.
        assert_eq!(search("dog or cat sort_by_desc=id").await, ["c", "b", "a"]);
        // Quoted text in the middle of a word is found, though the index can't rank it.
        assert_eq!(search("\"og\" or cat").await, ["b", "c", "a"]);

        // The index follows edits, keywords and deletes.
        let indexed = async |words: &str| {
            let mut ids = sqlx::query_scalar::<_, i64>(
                "SELECT rowid FROM note_fts WHERE note_fts MATCH ? ORDER BY rowid",
            )
            .bind(words)
            .fetch_all(&pool)
            .await
            .unwrap();
            ids.dedup();
            ids
        };
        exec(&format!(
            "UPDATE note SET data = 'a dog and a dog' WHERE id = {}",
            id("d")
        ))
        .await;
        assert_eq!(indexed("dog").await, [id("a"), id("b"), id("d")]);
        exec(&format!(
            "INSERT INTO note_keyword (note_id, keyword, embedded) VALUES ({}, 'Felines', 0)",
            id("c")
        ))
        .await;
        assert_eq!(indexed("felines").await, [id("c")]);
        assert_eq!(indexed("cat").await, [id("b"), id("c")]);
        exec(&format!("DELETE FROM note WHERE id = {}", id("c"))).await;
        assert_eq!(indexed("felines OR cat").await, [id("b")]);
        exec(&format!(
            "DELETE FROM note_keyword WHERE note_id = {}",
            id("c")
        ))
        .await;
        assert_eq!(indexed("cat").await, [id("b")]);
    }

    #[test]
    fn marks_where_the_text_matches() {
        let marks = |query: &str, text: &str| {
            let utf16 = text.encode_utf16().collect::<Vec<_>>();
            text_matches(&text_patterns(query), text)
                .into_iter()
                .map(|(start, end)| String::from_utf16(&utf16[start..end]).unwrap())
                .collect::<Vec<_>>()
        };
        let text = "Hot dogs: 12 cats, a fish, 😀 CAFÉ dog";
        // Touching matches are merged, and excluded terms and a link's query are not marked.
        assert_eq!(
            marks(
                r#"dog -cat "hot d" re:"\d+" linked_to_note=(fish) cafe"#,
                text
            ),
            ["Hot dog", "12", "CAFÉ", "dog"]
        );
        assert_eq!(marks("tag=math or (dog)", text), ["dog", "dog"]);
        assert_eq!(marks("og", text), Vec::<String>::new());
        assert_eq!(marks("dog (", text), Vec::<String>::new());
    }

    #[test]
    fn retrievability_bounds_turn_around() {
        let decay = 0.5;
        let ratio = |bound: &str| bound.split(' ').nth(1).unwrap().parse::<f64>().unwrap();
        let elapsed_over_stability =
            ratio(&retrievability_bound_with_decay(Op::LessThan, 0.8, decay));
        let recall =
            crate::schedulers::fsrs::utils::retrievability(elapsed_over_stability, 1.0, 0.5);
        assert!((recall - 0.8).abs() < 1e-6, "{recall}");
        assert_eq!(
            retrievability_bound_with_decay(Op::GreaterThan, 0.0, decay),
            "< 1e999"
        );
        assert_eq!(
            retrievability_bound_with_decay(Op::GreaterThanEqual, 1.0, decay),
            "<= 0"
        );
    }

    #[sqlx::test]
    async fn searches_custom_data(pool: SqlitePool) {
        sqlx::query("INSERT INTO parser (id, name) VALUES (1, 'markdown')")
            .execute(&pool)
            .await
            .unwrap();
        let notes = [
            ("a", r#"{"flag": true, "n": 0, "x": {"y": ["z", "zz"]}}"#),
            ("b", r#"{"flag": false, "s": ""}"#),
            ("c", "{}"),
            ("d", r#"{"flag": null, "a's key": 2}"#),
        ];
        for (letters, custom_data) in notes {
            sqlx::query(
                "INSERT INTO note (id, data, created_at, updated_at, custom_data, parser_id) VALUES (?, '', 0, 0, ?, 1)",
            )
            .bind(letters.parse::<NoteId>().unwrap())
            .bind(custom_data)
            .execute(&pool)
            .await
            .unwrap();
        }
        let search = async |query: &str| {
            Evaluator::new(query)
                .get_note_ids(&pool)
                .await
                .unwrap()
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(search("custom_data.flag").await, ["a"]);
        assert_eq!(search("-custom_data.flag").await, ["b", "c", "d"]);
        assert_eq!(search("not custom_data.flag").await, ["b", "c", "d"]);
        // `0` and `""` are there.
        assert_eq!(search("custom_data.n or custom_data.s").await, ["a", "b"]);
        assert_eq!(search("custom_data.x.y[1]=zz").await, ["a"]);
        assert_eq!(search("-custom_data.x.y[1]=zz").await, ["b", "c", "d"]);
        assert_eq!(search("custom_data.flag=false").await, ["b"]);
        assert_eq!(search(r#"custom_data["a's key"]>=1"#).await, ["d"]);
    }

    #[test]
    fn test_sorting() {
        let inputs = vec![
            // Basic ascending sort on note field
            (
                "sort_by_asc=created_at",
                "SELECT DISTINCT n.id FROM note n ORDER BY n.created_at ASC, n.id ASC",
            ),
            // Basic descending sort on note field
            (
                "sort_by_desc=updated_at",
                "SELECT DISTINCT n.id FROM note n ORDER BY n.updated_at DESC, n.id ASC",
            ),
            // Sort by note id
            (
                "sort_by_asc=id",
                "SELECT DISTINCT n.id FROM note n ORDER BY n.id ASC",
            ),
            // Sort by card field
            (
                "sort_by_asc=c.stability",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.stability ASC, n.id ASC",
            ),
            // Sort by card due date
            (
                "sort_by_desc=c.due",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.due DESC, n.id ASC",
            ),
            // Sort by numeric card fields
            (
                "sort_by_asc=c.difficulty",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.difficulty ASC, n.id ASC",
            ),
            (
                "sort_by_desc=c.desired_retention",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.desired_retention DESC, n.id ASC",
            ),
            // Sort by card id
            (
                "sort_by_asc=c.id",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.id ASC, n.id ASC",
            ),
            // Sort by card created_at
            (
                "sort_by_desc=c.created_at",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.created_at DESC, n.id ASC",
            ),
            // Sort by card count
            (
                "sort_by_asc=c.count",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY (SELECT COUNT(*) FROM card c2 WHERE c2.note_id = n.id) ASC, n.id ASC",
            ),
            // Several fields in one sort
            (
                "sort_by_asc=c.due,c.stability sort_by_desc=\"created_at, id\"",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.due ASC, c.stability ASC, n.created_at DESC, n.id DESC",
            ),
            // Multiple sorts
            (
                "sort_by_desc=c.stability sort_by_asc=c.id",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY c.stability DESC, c.id ASC, n.id ASC",
            ),
            // Sort combined with WHERE clause
            (
                "tag=math sort_by_asc=created_at",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE (EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%')) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%'))) ORDER BY n.created_at ASC, n.id ASC",
            ),
            (
                "c.stability>=2 sort_by_desc=c.due",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE c.stability >= 2 ORDER BY c.due DESC, n.id ASC",
            ),
            // Sort with linked_to_note field
            (
                "sort_by_asc=linked_to_note",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN note_link nl ON n.id = nl.parent_note_id ORDER BY n.id ASC",
            ),
            // Sort with linked_to_keyword field
            (
                "sort_by_asc=linked_to_keyword",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN note_link nl ON n.id = nl.parent_note_id ORDER BY (SELECT min(nl.searched_keyword) FROM note_link nl WHERE nl.parent_note_id = n.id) ASC, n.id ASC",
            ),
            // Sort with parenthesis
            (
                "(tag=math and c.stability>=2) sort_by_asc=linked_to_note",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id LEFT JOIN note_link nl ON n.id = nl.parent_note_id WHERE ((EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%')) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%'))) AND c.stability >= 2) ORDER BY n.id ASC",
            ),
            // The following should be equivalent to the one above.
            (
                "tag=math and (c.stability>=2 sort_by_asc=linked_to_note)",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id LEFT JOIN note_link nl ON n.id = nl.parent_note_id WHERE ((EXISTS (SELECT 1 FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%')) OR EXISTS (SELECT 1 FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE ct.card_id = c.id AND (t.name LIKE 'math' OR t.name LIKE 'math:%'))) AND c.stability >= 2) ORDER BY n.id ASC",
            ),
        ];

        for (input, expected_sql) in inputs {
            let evaluator = Evaluator::new(input);
            let query_str = evaluator
                .evaluate(EvaluatorReturnItemType::NoteIds)
                .unwrap();
            assert_eq!(
                query_str,
                expand_text(expected_sql),
                "Failed for input: {}",
                input
            );
        }
    }

    #[test]
    fn test_string_sorting() {
        let inputs = vec![
            // Sort by parser_name
            (
                "sort_by_asc=parser_name",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN parser p ON n.parser_id = p.id ORDER BY p.name ASC, n.id ASC",
            ),
            // Sort by keyword (subquery min)
            (
                "sort_by_desc=keyword",
                "SELECT DISTINCT n.id FROM note n ORDER BY (SELECT min(nk.keyword) FROM note_keyword nk WHERE nk.note_id = n.id) DESC, n.id ASC",
            ),
            // Sort by tag (subquery min)
            (
                "sort_by_asc=tag",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id ORDER BY (SELECT min(t.name) FROM note_tag nt JOIN tag t ON nt.tag_id = t.id WHERE nt.note_id = n.id) ASC, n.id ASC",
            ),
            // Sort by data
            (
                "sort_by_desc=data",
                "SELECT DISTINCT n.id FROM note n ORDER BY n.data DESC, n.id ASC",
            ),
        ];

        for (input, expected_sql) in inputs {
            let evaluator = Evaluator::new(input);
            let query_str = evaluator
                .evaluate(EvaluatorReturnItemType::NoteIds)
                .unwrap();
            assert_eq!(
                query_str,
                expand_text(expected_sql),
                "Failed for input: {}",
                input
            );
        }
    }

    #[test]
    fn test_limit() {
        let inputs = vec![
            // Basic limit
            (
                "limit=10",
                "SELECT DISTINCT n.id FROM note n ORDER BY n.id ASC LIMIT 10",
            ),
            // Limit combined with WHERE clause
            (
                "dog limit=5",
                "SELECT DISTINCT n.id FROM note n WHERE n.data REGEXP text(word dog) ORDER BY rank(\"dog\"*), n.id ASC LIMIT 5",
            ),
            // Limit combined with ORDER BY
            (
                "sort_by_asc=created_at limit=20",
                "SELECT DISTINCT n.id FROM note n ORDER BY n.created_at ASC, n.id ASC LIMIT 20",
            ),
            // Limit combined with WHERE and ORDER BY
            (
                "c.stability>=2 sort_by_desc=c.due limit=100",
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE c.stability >= 2 ORDER BY c.due DESC, n.id ASC LIMIT 100",
            ),
        ];

        for (input, expected_sql) in inputs {
            let evaluator = Evaluator::new(input);
            let query_str = evaluator
                .evaluate(EvaluatorReturnItemType::NoteIds)
                .unwrap();
            assert_eq!(
                query_str,
                expand_text(expected_sql),
                "Failed for input: {}",
                input
            );
        }
    }

    #[test]
    fn test_limit_per_or_branch() {
        let inputs = [
            // Two limited branches, card output uses review order by default
            (
                "(dog limit=5) or (c.state=0 limit=3)",
                EvaluatorReturnItemType::CardIds,
                "SELECT DISTINCT c.id FROM card c LEFT JOIN note n ON n.id = c.note_id WHERE (\
c.id IN (SELECT DISTINCT c.id FROM card c LEFT JOIN note n ON n.id = c.note_id WHERE n.data REGEXP text(word dog) ORDER BY c.due ASC, n.created_at ASC, c.id ASC LIMIT 5) OR \
c.id IN (SELECT DISTINCT c.id FROM card c LEFT JOIN note n ON n.id = c.note_id WHERE c.state = 0 /* new */ ORDER BY c.due ASC, n.created_at ASC, c.id ASC LIMIT 3)) ORDER BY c.due ASC, n.created_at ASC, c.id ASC",
            ),
            // A limited branch next to an unlimited one, note output orders by id by default
            (
                "(dog limit=5) or cat",
                EvaluatorReturnItemType::NoteIds,
                "SELECT DISTINCT n.id FROM note n WHERE (\
n.id IN (SELECT DISTINCT n.id FROM note n WHERE n.data REGEXP text(word dog) ORDER BY n.id ASC LIMIT 5) OR \
n.data REGEXP text(word cat)) ORDER BY rank(\"dog\"* OR \"cat\"*), n.id ASC",
            ),
            // A sort inside a limited branch picks which items the limit keeps, and a global
            // limit still caps the whole result
            (
                "(dog sort_by_desc=c.stability limit=2) or (cat limit=1) limit=10",
                EvaluatorReturnItemType::CardIds,
                "SELECT DISTINCT c.id FROM card c LEFT JOIN note n ON n.id = c.note_id WHERE (\
c.id IN (SELECT DISTINCT c.id FROM card c LEFT JOIN note n ON n.id = c.note_id WHERE n.data REGEXP text(word dog) ORDER BY c.stability DESC, c.id ASC LIMIT 2) OR \
c.id IN (SELECT DISTINCT c.id FROM card c LEFT JOIN note n ON n.id = c.note_id WHERE n.data REGEXP text(word cat) ORDER BY c.due ASC, n.created_at ASC, c.id ASC LIMIT 1)) ORDER BY c.due ASC, n.created_at ASC, c.id ASC LIMIT 10",
            ),
            // Limited groups can nest
            (
                "((dog limit=1) or cat limit=4) or (c.state=2 limit=2)",
                EvaluatorReturnItemType::CardIds,
                "SELECT DISTINCT c.id FROM card c LEFT JOIN note n ON n.id = c.note_id WHERE (\
c.id IN (SELECT DISTINCT c.id FROM card c LEFT JOIN note n ON n.id = c.note_id WHERE (\
c.id IN (SELECT DISTINCT c.id FROM card c LEFT JOIN note n ON n.id = c.note_id WHERE n.data REGEXP text(word dog) ORDER BY c.due ASC, n.created_at ASC, c.id ASC LIMIT 1) OR \
n.data REGEXP text(word cat)) ORDER BY c.due ASC, n.created_at ASC, c.id ASC LIMIT 4) OR \
c.id IN (SELECT DISTINCT c.id FROM card c LEFT JOIN note n ON n.id = c.note_id WHERE c.state = 2 /* review */ ORDER BY c.due ASC, n.created_at ASC, c.id ASC LIMIT 2)) ORDER BY c.due ASC, n.created_at ASC, c.id ASC",
            ),
        ];
        for (input, output_type, expected_sql) in inputs {
            let query_str = Evaluator::new(input).evaluate(output_type).unwrap();
            assert_eq!(
                query_str,
                expand_text(expected_sql),
                "Failed for input: {}",
                input
            );
        }
    }

    #[test]
    fn sorts_and_limits_apply_to_their_parentheses() {
        let sql = |input| {
            Evaluator::new(input)
                .evaluate(EvaluatorReturnItemType::NoteIds)
                .unwrap()
        };
        let whole = sql("(dog or cat) sort_by_asc=created_at limit=5");
        assert_eq!(sql("dog limit=5 or cat sort_by_asc=created_at"), whole);
        assert_eq!(sql("dog or cat sort_by_asc=created_at limit=5"), whole);
        assert_eq!(sql("dog or limit=5 cat sort_by_asc=created_at"), whole);
    }

    #[test]
    fn test_limit_with_scope() {
        // The scope is applied inside each limited branch, before its LIMIT, and to the whole
        // result, so unlimited branches are scoped too.
        let query_str = Evaluator::new("(dog limit=5) or cat")
            .with_scope("c.due <= 100".to_string())
            .evaluate(EvaluatorReturnItemType::CardIds)
            .unwrap();
        assert_eq!(
            query_str,
            expand_text(
                "SELECT DISTINCT c.id FROM card c LEFT JOIN note n ON n.id = c.note_id WHERE (\
c.id IN (SELECT DISTINCT c.id FROM card c LEFT JOIN note n ON n.id = c.note_id WHERE n.data REGEXP text(word dog) AND (c.due <= 100) ORDER BY c.due ASC, n.created_at ASC, c.id ASC LIMIT 5) OR \
n.data REGEXP text(word cat)) AND (c.due <= 100) ORDER BY c.due ASC, n.created_at ASC, c.id ASC"
            )
        );

        // Note output needs the card join for a card-level scope.
        let query_str = Evaluator::new("dog")
            .with_scope("c.due <= 100".to_string())
            .evaluate(EvaluatorReturnItemType::NoteIds)
            .unwrap();
        assert_eq!(
            query_str,
            expand_text(
                "SELECT DISTINCT n.id FROM note n LEFT JOIN card c ON n.id = c.note_id WHERE n.data REGEXP text(word dog) AND (c.due <= 100) ORDER BY rank(\"dog\"*), n.id ASC"
            )
        );
    }

    #[test]
    fn test_limited_group_cloze_error() {
        let query_str_res = Evaluator::new(r#"(c.cloze~"x" limit=5) or dog"#)
            .evaluate(EvaluatorReturnItemType::CardIds);
        assert!(query_str_res.is_err());
    }

    #[test]
    fn test_to_dnf_keeps_limited_group_whole() {
        let tree = Parser::new(r#"(dog or cat limit=3) c.cloze~"x""#)
            .parse_expression()
            .unwrap();
        // Distributing would split the limited group into `dog` and `cat limit=3`.
        assert_eq!(to_dnf(tree).len(), 1);
    }

    #[test]
    fn test_query_has_limit() {
        assert!(crate::search::query_has_limit("(dog limit=5) or cat"));
        assert!(crate::search::query_has_limit("dog limit=5"));
        assert!(!crate::search::query_has_limit("dog or cat"));
        assert!(!crate::search::query_has_limit("limited"));
        assert!(!crate::search::query_has_limit("(unterminated"));
    }

    #[test]
    fn test_limit_errors() {
        let inputs = [
            // Cannot use non-equal operator for limit
            "limit>=10",
            "limit~10",
            // Cannot negate limit
            "-limit=10",
            // limit can only be specified once
            "limit=10 limit=20",
            "(dog limit=5 limit=6) or cat",
            // Cannot negate a limited group
            "-(dog limit=5)",
        ];
        for input in inputs {
            let evaluator = Evaluator::new(input);
            let query_str_res = evaluator.evaluate(EvaluatorReturnItemType::NoteIds);
            assert!(
                query_str_res.is_err(),
                "Expected error for input: {}",
                input
            );
        }
    }

    #[test]
    fn test_sorting_errors() {
        let inputs = [
            // Cannot sort by boolean fields
            "sort_by_asc=c.suspended",
            "sort_by_desc=c.user_buried",
            // Cannot sort by json fields
            "sort_by_desc=custom_data",
            // Cannot use non-equal operator for sorting
            "sort_by_asc>=created_at",
            "sort_by_desc~c.stability",
            // A card has many ratings
            "sort_by_asc=c.rated",
            // A field missing from a list
            "sort_by_asc=c.due,",
            "sort_by_asc=c.due,,id",
            "sort_by_asc=c.due,c.suspended",
            // Invalid field name
            "sort_by_asc=invalid_field",
            "sort_by_asc=sort_by_asc",
            "sort_by_asc=sort_by_desc",
            // Cannot use certain operators for sorting
            "-sort_by_asc=linked_to_note",
            // `and` is fine since that is implictly used everywhere
            // Cannot use sort by inside grouping
            "c.stability>=2 or (a sort_by_asc=linked_to_note)",
            // NOTE: Even though this is equivalent to `(c.stability >=2 or a) sort_by_asc=linked_to_note`, this creates more confusion, so it is not allowed at the moment.
            "c.stability>=2 -(a sort_by_asc=linked_to_note)",
        ];
        for input in inputs {
            let evaluator = Evaluator::new(input);
            let query_str_res = evaluator.evaluate(EvaluatorReturnItemType::NoteIds);
            assert!(
                query_str_res.is_err(),
                "Expected error for input: {}",
                input
            );
        }
    }

    #[test]
    fn test_cloze_field_sql_generation() {
        // c.cloze conditions emit 1 (tautology) in SQL; other conditions still apply.
        // The parser join is needed when fetching cards for cloze filtering.
        let inputs = vec![
            (
                r#"c.cloze~"proof""#,
                "SELECT DISTINCT c.id FROM card c LEFT JOIN note n ON n.id = c.note_id WHERE 1 ORDER BY c.due ASC, n.created_at ASC, c.id ASC",
            ),
            (
                r#"c.cloze~"proof" AND c.suspended=false"#,
                "SELECT DISTINCT c.id FROM card c LEFT JOIN note n ON n.id = c.note_id WHERE (1 AND (c.special_state IS NULL OR c.special_state != 1)) ORDER BY c.due ASC, n.created_at ASC, c.id ASC",
            ),
        ];
        for (input, expected_sql) in inputs {
            let evaluator = Evaluator::new(input);
            let query_str = evaluator
                .evaluate(EvaluatorReturnItemType::CardIds)
                .unwrap();
            assert_eq!(
                query_str,
                expand_text(expected_sql),
                "Failed for input: {}",
                input
            );
        }
    }

    #[test]
    fn test_has_cloze_field() {
        let tree = Evaluator::new(r#"c.cloze~"test""#)
            .parser
            .parse_expression()
            .unwrap();
        assert!(has_cloze_field(&tree));

        let tree = Evaluator::new("c.stability>=2")
            .parser
            .parse_expression()
            .unwrap();
        assert!(!has_cloze_field(&tree));
    }

    #[test]
    fn test_extract_cloze_patterns() {
        let tree = Evaluator::new(r#"c.cloze~"proof""#)
            .parser
            .parse_expression()
            .unwrap();
        let patterns = extract_cloze_patterns(&tree);
        assert_eq!(patterns.len(), 1);
        assert!(patterns[0].matches("a proof of X"));
        assert!(patterns[0].matches("a PRÖOF of X"));
        assert!(!patterns[0].matches("a lemma"));

        // Without quotes, it is the start of a word.
        let tree = Parser::new("c.cloze~roof").parse_expression().unwrap();
        let patterns = extract_cloze_patterns(&tree);
        assert!(patterns[0].matches("a roofer"));
        assert!(!patterns[0].matches("a proof"));

        let tree = Evaluator::new(r#"c.cloze~re:"^#cl\[""#)
            .parser
            .parse_expression()
            .unwrap();
        let patterns = extract_cloze_patterns(&tree);
        assert_eq!(patterns.len(), 1);
        assert!(patterns[0].matches("#cl[hidden]"));
        assert!(!patterns[0].matches("not a cloze"));
    }

    #[test]
    fn test_to_dnf() {
        // Simple literal — stays as one disjunct.
        let tree = Evaluator::new(r#"c.cloze~"proof""#)
            .parser
            .parse_expression()
            .unwrap();
        let disjuncts = to_dnf(tree);
        assert_eq!(disjuncts.len(), 1);

        // AND — stays as one conjunction, not split.
        let tree = Evaluator::new(r#"c.cloze~"proof" AND c.suspended=false"#)
            .parser
            .parse_expression()
            .unwrap();
        let disjuncts = to_dnf(tree);
        assert_eq!(disjuncts.len(), 1);

        // OR — two disjuncts.
        let tree = Evaluator::new(r#"c.cloze~"a" OR c.cloze~"b""#)
            .parser
            .parse_expression()
            .unwrap();
        let disjuncts = to_dnf(tree);
        assert_eq!(disjuncts.len(), 2);

        // (A AND c.cloze~b) OR C — two disjuncts.
        let tree = Evaluator::new(r#"(c.stability>=2 AND c.cloze~"proof") OR c.suspended=false"#)
            .parser
            .parse_expression()
            .unwrap();
        let disjuncts = to_dnf(tree);
        assert_eq!(disjuncts.len(), 2);
        // First disjunct contains cloze, second does not.
        assert!(has_cloze_field(&disjuncts[0]));
        assert!(!has_cloze_field(&disjuncts[1]));
    }

    #[test]
    fn test_cloze_sql_per_disjunct() {
        // Verify that each DNF disjunct produces the expected SQL.
        // OR query: each branch should generate its own SQL.
        let tree = Evaluator::new(r#"(c.stability>=2 AND c.cloze~"proof") OR c.suspended=false"#)
            .parser
            .parse_expression()
            .unwrap();
        let disjuncts = to_dnf(tree);
        assert_eq!(disjuncts.len(), 2);

        let sql0 = Evaluator::build_query_from_tree(
            &disjuncts[0],
            EvaluatorReturnItemType::CardIds,
            false,
            None,
        )
        .unwrap();
        // Cloze becomes "1", stability condition stays.
        assert!(
            sql0.contains("c.stability"),
            "Expected stability in SQL: {sql0}"
        );
        assert!(
            !sql0.contains("proof"),
            "Cloze value must not appear in SQL: {sql0}"
        );

        let sql1 = Evaluator::build_query_from_tree(
            &disjuncts[1],
            EvaluatorReturnItemType::CardIds,
            false,
            None,
        )
        .unwrap();
        // Second disjunct is c.suspended=false — no cloze tautology at the WHERE level.
        assert!(
            sql1.contains("special_state"),
            "Expected suspended check in SQL: {sql1}"
        );
        assert!(
            !sql1.contains("WHERE 1"),
            "No bare tautology expected in cloze-free disjunct: {sql1}"
        );
    }
}
