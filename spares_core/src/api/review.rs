use std::collections::HashMap;
use std::fmt::Write;
use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Duration;
use chrono::Local;
use chrono::NaiveDate;
use chrono::Utc;
use indoc::formatdoc;
use indoc::indoc;
use itertools::Itertools;
use log::info;
use log::warn;
use serde_json::Value;
use serde_json::to_value;
use sqlx::FromRow;
use sqlx::sqlite::SqlitePool;

use super::note::delete_empty_tags;
use crate::ALLOWED_F64_ERROR;
use crate::Error;
use crate::LibraryError;
use crate::TagErrorKind;
use crate::api::browser_render::add_browser_render_sources;
use crate::api::browser_render::load_review_card;
use crate::api::card::create_card_tags;
use crate::api::card::delete_card_tags;
use crate::api::card::unbury_cards;
use crate::api::fetch_review_logs_for_replay;
use crate::api::note::update_notes;
use crate::api::preset::CardConfigs;
use crate::api::preset::PresetLimitsLeft;
use crate::api::preset::cap_counts;
use crate::api::preset::limited_preset_sql;
use crate::api::preset::preset_index;
use crate::api::queue::QueueCaps;
use crate::api::queue::queue_order_by;
use crate::api::tag::create_tag;
use crate::api::undo::insert_events;
use crate::api::undo::join_event_group;
use crate::api::undo::payloads::FilteredTagExit;
use crate::api::undo::payloads::RateCardPayload;
use crate::api::undo::payloads::Transition;
use crate::api::undo::payloads::UpdateCardPayload;
use crate::api::validate_bury_target;
use crate::config::LeechAction;
use crate::config::LeechConfig;
use crate::config::QueueConfig;
use crate::config::QueuePreset;
use crate::config::SparesExternalConfig;
use crate::config::get_files_dir;
use crate::config::read_external_config;
use crate::config::read_internal_config;
use crate::config::resolve_in_files_dir;
use crate::config::write_internal_config;
use crate::helpers::get_start_end_local_date;
use crate::ids::IdTable;
use crate::ids::next_id_in;
use crate::model::Card;
use crate::model::CardId;
use crate::model::EventType;
use crate::model::NEW_CARD_STATE;
use crate::model::NoteId;
use crate::model::RatingId;
use crate::model::ReviewLog;
use crate::model::ReviewLogKind;
use crate::model::SpecialState;
use crate::model::StateId;
use crate::model::Tag;
use crate::model::TagId;
use crate::parsers::BackType;
use crate::parsers::Parseable;
use crate::parsers::RenderOutputDirectoryType;
use crate::parsers::audio::card_audio;
use crate::parsers::cli::compute_surrounding_text;
use crate::parsers::cli::parse_cli_data;
use crate::parsers::find_parser;
use crate::parsers::generate_files::CardSide;
use crate::parsers::generate_files::RenderOutputType;
use crate::parsers::get_all_parsers;
use crate::parsers::get_output_raw_dir;
use crate::parsers::has_web_render_file;
use crate::parsers::typed_answer::typed_answer_info;
use crate::parsers::web_render_filepath;
use crate::schedulers::SrsScheduler;
use crate::schedulers::effective_review_logs;
use crate::schedulers::get_scheduler_from_string;
use crate::schedulers::learning_states;
use crate::schedulers::placement::place_due;
use crate::schedulers::review_state;
use crate::schema::note::NotesSelector;
use crate::schema::note::UpdateNotesRequest;
use crate::schema::note::UpdateTags;
use crate::schema::review::CardBackRenderedPath;
use crate::schema::review::CliReviewInfo;
use crate::schema::review::GetReviewCardFilterRequest;
use crate::schema::review::GetReviewCardRequest;
use crate::schema::review::GetReviewCardResponse;
use crate::schema::review::RatingSubmission;
use crate::schema::review::ReviewLinkedNote;
use crate::schema::review::ReviewSnapshotRequest;
use crate::schema::review::ReviewSnapshotResponse;
use crate::schema::review::StudyAction;
use crate::schema::review::SubmitStudyActionRequest;
use crate::schema::review::SubmitStudyActionResponse;
use crate::schema::tag::CreateTagRequest;
use crate::search::evaluator::Evaluator;
/// Makes a path relative to the [files directory](get_files_dir), when it is inside it.
pub(crate) fn relative_to_files_dir(path: PathBuf) -> PathBuf {
    match path.strip_prefix(get_files_dir()) {
        Ok(relative) => relative.to_path_buf(),
        Err(_) => path,
    }
}

/// The HTML for web clients next to `rendered_path` (relative to the files directory when inside
/// it), in the same form, if there is HTML to show.
pub(crate) fn web_render_path(rendered_path: &Path) -> Option<PathBuf> {
    has_web_render_file(&resolve_in_files_dir(rendered_path))
        .then(|| web_render_filepath(rendered_path))
}

#[derive(Clone, Debug, FromRow)]
pub(crate) struct ReviewCard {
    note_id: NoteId,
    parser_name: String,
    card_order: u32,
    card_back_type: BackType,
    pub(crate) card_id: i64,
    pub(crate) card_state: StateId,
}

/// The columns of a [`ReviewCard`], for a query on `card c JOIN note n JOIN parser p`.
pub(crate) const REVIEW_CARD_COLUMNS: &str = r#"n.id as note_id,
            p.name as parser_name,
            c."order" as card_order,
            c.back_type as card_back_type,
            c.id as card_id,
            c.state as card_state"#;

/// Fetch the raw note text for a CLI card. Only called when the card's
/// `back_type` is [`BackType::Cli`]; non-CLI cards skip this query entirely.
async fn fetch_note_data_for_cli(db: &SqlitePool, note_id: NoteId) -> Result<String, Error> {
    sqlx::query_scalar(r"SELECT data FROM note WHERE id = ?")
        .bind(note_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
}

#[derive(Clone, Debug, FromRow)]
struct LinkedNoteRow {
    searched_keyword: String,
    linked_note_id: NoteId,
    matched_keyword: Option<String>,
    parser_name: String,
}

async fn get_linked_notes_for_review(
    db: &SqlitePool,
    note_id: NoteId,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<Vec<ReviewLinkedNote>, Error> {
    let rows: Vec<LinkedNoteRow> = sqlx::query_as(
        r"SELECT nl.searched_keyword, nl.linked_note_id, nl.matched_keyword, p.name as parser_name
          FROM note_link nl
          JOIN note n ON nl.linked_note_id = n.id
          JOIN parser p ON n.parser_id = p.id
          WHERE nl.parent_note_id = ?
          AND nl.linked_note_id IS NOT NULL",
    )
    .bind(note_id)
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    let mut linked_notes = Vec::new();
    for row in rows {
        let parser = find_parser(&row.parser_name, all_parsers)?;
        let mut note_raw_path =
            get_output_raw_dir(parser.get_parser_name(), RenderOutputType::Note, None);
        note_raw_path.push(parser.get_output_filename(RenderOutputType::Note, row.linked_note_id));
        note_raw_path.set_extension(parser.file_extension());
        linked_notes.push(ReviewLinkedNote {
            searched_keyword: row.searched_keyword,
            note_id: row.linked_note_id,
            matched_keyword: row.matched_keyword,
            note_raw_path: relative_to_files_dir(note_raw_path),
        });
    }
    Ok(linked_notes)
}

#[expect(clippy::too_many_lines)]
fn build_review_card_response(
    ReviewCard {
        note_id,
        parser_name,
        card_order,
        card_back_type,
        card_id,
        card_state,
    }: ReviewCard,
    note_data: Option<String>,
    cards_left_by_state: HashMap<StateId, u32>,
    time_estimate: Duration,
    all_parsers: &[fn() -> Box<dyn Parseable>],
    linked_notes: Vec<ReviewLinkedNote>,
    keywords: Vec<String>,
) -> Result<GetReviewCardResponse, Error> {
    let parser = find_parser(parser_name.as_str(), all_parsers)?;

    let mut card_front_rendered_path =
        parser.get_output_rendered_dir(RenderOutputDirectoryType::Card);
    card_front_rendered_path.push(parser.get_output_filename(
        RenderOutputType::Card(card_order as usize, CardSide::Front),
        note_id,
    ));
    let card_front_rendered_path = relative_to_files_dir(card_front_rendered_path);

    let mut card_front_raw_path = get_output_raw_dir(
        parser.get_parser_name(),
        RenderOutputType::Card(card_order as usize, CardSide::Front),
        None,
    );
    card_front_raw_path.push(parser.get_output_filename(
        RenderOutputType::Card(card_order as usize, CardSide::Front),
        note_id,
    ));
    card_front_raw_path.set_extension(parser.file_extension());
    let card_front_raw_path = relative_to_files_dir(card_front_raw_path);

    let mut note_raw_path =
        get_output_raw_dir(parser.get_parser_name(), RenderOutputType::Note, None);
    note_raw_path.push(parser.get_output_filename(RenderOutputType::Note, note_id));
    note_raw_path.set_extension(parser.file_extension());
    let note_raw_path = relative_to_files_dir(note_raw_path);

    let card_back_rendered_path = match card_back_type {
        BackType::NoteFilePath | BackType::Cli => {
            let mut note_rendered_path =
                parser.get_output_rendered_dir(RenderOutputDirectoryType::Note);
            note_rendered_path.push(parser.get_output_filename(RenderOutputType::Note, note_id));
            CardBackRenderedPath::Note(relative_to_files_dir(note_rendered_path))
        }
        BackType::CardFilePath => {
            let mut card_back_rendered_path =
                parser.get_output_rendered_dir(RenderOutputDirectoryType::Card);
            card_back_rendered_path.push(parser.get_output_filename(
                RenderOutputType::Card(card_order as usize, CardSide::Back),
                note_id,
            ));
            CardBackRenderedPath::CardBack(relative_to_files_dir(card_back_rendered_path))
        }
    };

    let card_back_raw_path = match card_back_type {
        BackType::NoteFilePath | BackType::Cli => {
            let mut path =
                get_output_raw_dir(parser.get_parser_name(), RenderOutputType::Note, None);
            path.push(parser.get_output_filename(RenderOutputType::Note, note_id));
            path.set_extension(parser.file_extension());
            CardBackRenderedPath::Note(relative_to_files_dir(path))
        }
        BackType::CardFilePath => {
            let mut path = get_output_raw_dir(
                parser.get_parser_name(),
                RenderOutputType::Card(card_order as usize, CardSide::Back),
                None,
            );
            path.push(parser.get_output_filename(
                RenderOutputType::Card(card_order as usize, CardSide::Back),
                note_id,
            ));
            path.set_extension(parser.file_extension());
            CardBackRenderedPath::CardBack(relative_to_files_dir(path))
        }
    };

    // CLI cards: re-parse the note's `data` for `spares: cli start…end` blocks
    // and surface the block matching this card's `order` (1-based) so the
    // review loop can spawn `exec` and read a score from its stdout. Only
    // CLI cards have `BackType::Cli`; for everything else `cli` is `None`.
    let cli = if matches!(card_back_type, BackType::Cli) {
        let note_data = note_data.ok_or_else(|| {
            Error::Library(LibraryError::Card(crate::CardErrorKind::InvalidInput(
                format!(
                    "Note {} card {} is a CLI card (BackType::Cli) but note data was not loaded.",
                    note_id.letters(),
                    card_order
                ),
            )))
        })?;
        let cli_blocks = parse_cli_data(parser.as_ref(), &note_data)?;
        let cli_block = cli_blocks
            .get(
                card_order
                    .checked_sub(1)
                    .and_then(|i| usize::try_from(i).ok())
                    .unwrap_or(usize::MAX),
            )
            .map(|(cli_data, _range)| cli_data)
            .ok_or_else(|| {
                Error::Library(LibraryError::Card(crate::CardErrorKind::InvalidInput(
                    format!(
                        "Note {} card {} is a CLI card (BackType::Cli) but no \
                         matching `spares: cli start` block was found in note data.",
                        note_id.letters(),
                        card_order
                    ),
                )))
            })?;
        Some(CliReviewInfo {
            exec: cli_block.exec.clone(),
            surrounding: compute_surrounding_text(&note_data, &cli_blocks),
        })
    } else {
        None
    };

    // Both sides are shown in the same format, so HTML is used only if both sides have it.
    let web_paths = if parser.has_web_render() {
        let card_back_web_path = match &card_back_rendered_path {
            CardBackRenderedPath::CardBack(path) => {
                web_render_path(path).map(CardBackRenderedPath::CardBack)
            }
            CardBackRenderedPath::Note(path) => {
                web_render_path(path).map(CardBackRenderedPath::Note)
            }
        };
        web_render_path(&card_front_rendered_path).zip(card_back_web_path)
    } else {
        None
    };
    let (card_front_web_path, card_back_web_path) = web_paths.unzip();

    Ok(GetReviewCardResponse {
        note_id,
        card_order,
        card_id,
        card_state,
        card_front_rendered_path,
        card_back_rendered_path,
        card_front_web_path,
        card_back_web_path,
        card_front_raw_path,
        card_back_raw_path,
        note_raw_path,
        parser_name,
        keywords,
        cli,
        browser_sources: None,
        typed_answer: None,
        audio: None,
        cards_left_by_state,
        time_estimate,
        linked_notes,
    })
}

const DEFAULT_ESTIMATED_CARD_REVIEW_SECONDS: f64 = 30.0;

/// What is left of today's limits on `requested_date`'s local day. See
/// [`SparesExternalConfig::new_cards_daily_limit`] and
/// [`SparesExternalConfig::reviews_daily_limit`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DailyLimitsLeft {
    pub(crate) new: u32,
    pub(crate) reviews: u32,
    /// What is left of each [preset](crate::config::QueuePreset)'s own limits, by its index in
    /// [`QueueConfig::presets`]. Empty when no preset has limits.
    pub(crate) presets: Vec<PresetLimitsLeft>,
}

pub(crate) async fn daily_limits_left(
    db: &SqlitePool,
    requested_date: DateTime<Utc>,
) -> Result<DailyLimitsLeft, Error> {
    daily_limits_left_with(db, &read_external_config()?, requested_date).await
}

/// See [`DailyLimitsLeft`].
///
/// A preset's limits work like the top-level ones, on its own cards: with
/// `reviews_daily_limit = 100` on a preset whose 80 reviews are due, and
/// `new_cards_ignore_review_limit` off, at most 20 of its new cards are shown, whatever is left of
/// the top-level limits. The top-level new card limit only counts the reviews due that the
/// presets' review limits let through, so a preset holding back its reviews leaves room for new
/// cards elsewhere.
#[expect(clippy::too_many_lines)]
pub(crate) async fn daily_limits_left_with(
    db: &SqlitePool,
    config: &SparesExternalConfig,
    requested_date: DateTime<Utc>,
) -> Result<DailyLimitsLeft, Error> {
    let (lower_limit, upper_limit) = get_start_end_local_date(&requested_date);
    // The preset is only needed for the presets' own limits
    let preset = limited_preset_sql(&config.queue);
    // `LEFT JOIN`, so that the reviews of a deleted card still count, with no preset
    let studied: Vec<(Option<i64>, StateId, u32)> = sqlx::query_as(&format!(
        r"SELECT {preset} AS preset, rl.previous_state, COUNT(DISTINCT rl.card_id)
        FROM review_log rl LEFT JOIN card c ON c.id = rl.card_id
        WHERE rl.reviewed_at >= ? AND rl.reviewed_at <= ? AND rl.kind = ?
        AND rl.previous_state IN (?, ?)
        GROUP BY 1, 2"
    ))
    .bind(lower_limit.timestamp())
    .bind(upper_limit.timestamp())
    // Forgetting a card must not use up one of today's cards.
    .bind(ReviewLogKind::Review)
    .bind(NEW_CARD_STATE)
    .bind(review_state())
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let studied_from = |state: StateId, of_preset: Option<Option<usize>>| -> u32 {
        studied
            .iter()
            .filter(|(preset, studied_state, _)| {
                *studied_state == state
                    && of_preset.is_none_or(|of_preset| preset_index(*preset) == of_preset)
            })
            .map(|(_, _, count)| count)
            .sum()
    };
    let new_studied = studied_from(NEW_CARD_STATE, None);
    let reviews_studied = studied_from(review_state(), None);
    let ignore_review_limit = config.new_cards_ignore_review_limit;

    let reviews_due: Vec<(Option<i64>, u32)> = if ignore_review_limit {
        Vec::new()
    } else {
        sqlx::query_as(&format!(
            r"SELECT {preset} AS preset, COUNT(*) FROM card c
            WHERE c.state = ? AND c.special_state IS NULL AND c.due <= ?
            GROUP BY 1"
        ))
        .bind(review_state())
        .bind(upper_limit.timestamp())
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?
    };
    let reviews_due_of = |of_preset: Option<usize>| -> u32 {
        reviews_due
            .iter()
            .filter(|(preset, _)| preset_index(*preset) == of_preset)
            .map(|(_, count)| count)
            .sum()
    };

    let has_preset_limits = config.queue.presets.values().any(QueuePreset::has_limits);
    let presets = if has_preset_limits {
        config
            .queue
            .presets
            .values()
            .enumerate()
            .map(|(index, preset)| {
                let new_studied = studied_from(NEW_CARD_STATE, Some(Some(index)));
                let reviews_studied = studied_from(review_state(), Some(Some(index)));
                let counted = if ignore_review_limit {
                    reviews_studied
                } else {
                    reviews_studied + new_studied
                };
                let reviews = preset
                    .reviews_daily_limit
                    .map(|limit| limit.saturating_sub(counted));
                let mut new = preset
                    .new_cards_daily_limit
                    .map(|limit| limit.saturating_sub(new_studied));
                if !ignore_review_limit && let Some(reviews) = reviews {
                    let room = reviews.saturating_sub(reviews_due_of(Some(index)));
                    new = Some(new.map_or(room, |new| new.min(room)));
                }
                PresetLimitsLeft { new, reviews }
            })
            .collect()
    } else {
        Vec::new()
    };

    let mut new = config.new_cards_daily_limit.saturating_sub(new_studied);
    if ignore_review_limit {
        let reviews = config.reviews_daily_limit.saturating_sub(reviews_studied);
        return Ok(DailyLimitsLeft {
            new,
            reviews,
            presets,
        });
    }
    let reviews = config
        .reviews_daily_limit
        .saturating_sub(reviews_studied + new_studied);
    // New cards only fill the room the reviews due today leave under the review limit, counting
    // only the reviews the presets' own limits let through
    let reviews_due: u32 = reviews_due
        .iter()
        .map(|(preset, count)| {
            preset_index(*preset)
                .and_then(|index| presets.get(index))
                .and_then(|left| left.reviews)
                .map_or(*count, |left| (*count).min(left))
        })
        .sum();
    new = new.min(reviews.saturating_sub(reviews_due));
    Ok(DailyLimitsLeft {
        new,
        reviews,
        presets,
    })
}

/// The conditions that leave out new cards once today's new card limit is used up, and review
/// cards once the review limit is, each starting with `"\nAND "`. Empty while there is room.
///
/// Likewise for each preset's own limits, with `preset`, the SQL of a card's preset (see
/// [`limited_preset_sql`]): once the `languages` preset's new card limit is used up, its new cards
/// are left out, while other new cards are still shown.
pub(crate) fn daily_limits_clause(left: &DailyLimitsLeft, preset: &str) -> String {
    let mut clause = String::new();
    for (left, state) in [(left.new, NEW_CARD_STATE), (left.reviews, review_state())] {
        if left == 0 {
            let _ = write!(clause, "\nAND c.state != {state}");
        }
    }
    for state in [NEW_CARD_STATE, review_state()] {
        let used_up = left
            .presets
            .iter()
            .positions(|left| left.of_state(state) == Some(0))
            .join(", ");
        if !used_up.is_empty() {
            // A card of no preset is `NULL`, which `IN` would make the whole condition
            let _ = write!(
                clause,
                "\nAND NOT (c.state = {state} AND COALESCE({preset}, -1) IN ({used_up}))"
            );
        }
    }
    clause
}

/// The cards an unfiltered review would show on `requested_date`, as a SQL condition with the due
/// limit inlined. Matches the default `where_clause` built in [`build_review_filter_query`].
async fn review_eligibility_clause(
    db: &SqlitePool,
    requested_date: DateTime<Utc>,
) -> Result<String, Error> {
    let (_, card_due_limit) = get_start_end_local_date(&requested_date);
    let config = read_external_config()?;
    let not_new_card_str = daily_limits_clause(
        &daily_limits_left_with(db, &config, requested_date).await?,
        &limited_preset_sql(&config.queue),
    );
    let buried_state = SpecialState::BuriedUntilLaterToday as u8;
    Ok(format!(
        "(c.special_state IS NULL AND c.due <= {}{not_new_card_str}) OR (c.special_state = {buried_state})",
        card_due_limit.timestamp()
    ))
}

/// Name of the filtered tag that holds the review snapshot for `query`. Whitespace is normalized
/// so trivially different spellings of a query share a snapshot.
fn review_snapshot_tag_name(query: &str) -> String {
    let normalized = query.split_whitespace().join(" ");
    let hash = blake3::hash(normalized.as_bytes()).to_hex();
    format!("review-{}", &hash[..8])
}

/// Resolves `query` against the cards due on `requested_date`, applying any per-group limits, and
/// stores the result in a filtered tag that review then works through with
/// [`GetReviewCardFilterRequest::FilteredTag`].
///
/// A query always maps to the same tag. The first call on a new local day rebuilds the tag's card
/// set; later calls that day return it unchanged, so a session can be resumed, and finishing it
/// doesn't make room for more cards.
pub async fn create_review_snapshot_tag(
    db: &SqlitePool,
    body: ReviewSnapshotRequest,
    requested_date: DateTime<Utc>,
) -> Result<ReviewSnapshotResponse, Error> {
    let ReviewSnapshotRequest { query } = body;
    let tag_name = review_snapshot_tag_name(&query);
    let existing_tag: Option<Tag> = sqlx::query_as(r"SELECT * FROM tag WHERE name = ?")
        .bind(&tag_name)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    let (start_of_day, _) = get_start_end_local_date(&requested_date);
    let (tag_id, rebuilt) = match existing_tag {
        Some(tag) if tag.updated_at >= start_of_day => (tag.id, false),
        Some(tag) => (tag.id, true),
        None => {
            // `auto_delete` stays off: if finishing the snapshot deleted the tag, running the
            // same query again that day would build a fresh set and exceed the limits.
            let tag = create_tag(
                db,
                CreateTagRequest {
                    name: tag_name.clone(),
                    description: format!("Review snapshot for `{query}`"),
                    query: Some(query.clone()),
                    auto_delete: false,
                    reschedule: true,
                },
                true,
            )
            .await?;
            (tag.id, true)
        }
    };

    if rebuilt {
        let scope = review_eligibility_clause(db, requested_date).await?;
        let card_ids = Evaluator::new(&query)
            .with_scope(scope)
            .get_card_ids(db)
            .await?;
        // Leftovers from an earlier day are dropped along with their filtered-tag scheduler
        // data, as happens when a card finishes a filtered tag. They come back if still due.
        sqlx::query(
            r"UPDATE card SET custom_data = json_remove(custom_data, '$.' || json_quote(?))
            WHERE json_extract(custom_data, '$.' || json_quote(?)) IS NOT NULL",
        )
        .bind(tag_id.to_string())
        .bind(tag_id.to_string())
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
        sqlx::query(r"DELETE FROM card_tag WHERE tag_id = ?")
            .bind(tag_id)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        let card_tag_entries = card_ids
            .into_iter()
            .map(|card_id| (card_id, tag_id))
            .collect::<Vec<_>>();
        create_card_tags(db, &card_tag_entries).await?;
        sqlx::query(r"UPDATE tag SET updated_at = ? WHERE id = ?")
            .bind(requested_date.timestamp())
            .bind(tag_id)
            .execute(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    }

    let card_count: u32 = sqlx::query_scalar(r"SELECT COUNT(*) FROM card_tag WHERE tag_id = ?")
        .bind(tag_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(ReviewSnapshotResponse {
        tag_id,
        tag_name,
        rebuilt,
        card_count,
    })
}

pub(crate) struct ReviewFilterQuery {
    /// Binds `card_due_limit` unless `is_filtered_tag`.
    pub(crate) where_clause: String,
    pub(crate) is_filtered_tag: bool,
    pub(crate) card_due_limit: DateTime<Utc>,
    /// What is left of today's limits. Filtered tags have no limits.
    pub(crate) caps: QueueCaps,
    /// The SQL of a card's preset, for the presets' own limits in `caps` (see
    /// [`limited_preset_sql`]).
    pub(crate) preset_sql: String,
    /// The learning and relearning cards due later today that match the filter but are held back
    /// by [`QueueConfig::learn_ahead_limit`](crate::config::QueueConfig::learn_ahead_limit), as a
    /// SQL condition with nothing to bind. `None` for filtered tags, which hold nothing back.
    pub(crate) held_back_clause: Option<String>,
    /// The sort the query (or the filtered tag's query) asks for, for [`queue_order_by`].
    pub(crate) query_sort: Vec<String>,
}

/// Build the `where_clause`, `is_filtered_tag` flag, and `card_due_limit` for a review filter.
///
/// With `learn_ahead`, learning and relearning cards are only included once their step ends
/// within the learn ahead limit, as review shows them. Without it, they are included whenever
/// they are due, for clients that pick the time to show them themselves.
///
/// Returns `Ok(None)` when a `FilteredTag` filter references a tag that does not exist.
/// Returns `Err` when a `FilteredTag` filter references a tag that has no query.
/// Returns `Ok(Some(…))` on success.
pub(crate) async fn build_review_filter_query(
    db: &SqlitePool,
    filter: Option<&GetReviewCardFilterRequest>,
    requested_date: DateTime<Utc>,
    learn_ahead: bool,
) -> Result<Option<ReviewFilterQuery>, Error> {
    let (_, card_due_limit) = get_start_end_local_date(&requested_date);
    let config = read_external_config()?;
    let limits_left = daily_limits_left_with(db, &config, requested_date).await?;
    let preset_sql = limited_preset_sql(&config.queue);
    let not_new_card_str = daily_limits_clause(&limits_left, &preset_sql);
    let mut query_sort = Vec::new();
    let card_id_query_str = if let Some(GetReviewCardFilterRequest::Query(query)) = filter {
        query_sort = Evaluator::new(query).card_order_by()?;
        let evaluator = Evaluator::new(query);
        let card_ids_str = evaluator.get_card_ids(db).await?.into_iter().join(", ");
        format!("\nAND c.id IN ({})", card_ids_str)
    } else {
        String::new()
    };
    let is_filtered_tag = matches!(filter, Some(GetReviewCardFilterRequest::FilteredTag { .. }));
    let buried_state = SpecialState::BuriedUntilLaterToday as u8;
    let learning_states = learning_states().iter().join(", ");
    let learn_ahead_until = (requested_date + config.queue.learn_ahead_limit).timestamp();
    let where_clause = if let Some(GetReviewCardFilterRequest::FilteredTag { tag_id }) = filter {
        let tag_query_opt: Option<(Option<String>,)> =
            sqlx::query_as(r"SELECT query FROM tag WHERE id = ?")
                .bind(tag_id)
                .fetch_optional(db)
                .await
                .map_err(|e| Error::Sqlx { source: e })?;
        if tag_query_opt.is_none() {
            return Ok(None);
        }
        let Some((Some(tag_query),)) = tag_query_opt else {
            return Err(Error::Library(LibraryError::Tag(
                TagErrorKind::InvalidInput(
                    "Cannot study a tag that does not have a query.".to_string(),
                ),
            )));
        };
        query_sort = Evaluator::new(&tag_query).card_order_by()?;
        format!(
            "(c.special_state IS NULL OR c.special_state = {buried_state})\n    AND c.id IN (SELECT ct.card_id FROM card_tag ct JOIN tag t ON ct.tag_id = t.id WHERE t.id = {tag_id})"
        )
    } else {
        let learn_ahead_str = if learn_ahead {
            format!(
                "\n    AND (c.state NOT IN ({learning_states}) OR c.due <= {learn_ahead_until})"
            )
        } else {
            String::new()
        };
        format!(
            "((c.special_state IS NULL AND c.due <= ?{not_new_card_str})\n    OR (c.special_state = {buried_state})){card_id_query_str}{learn_ahead_str}"
        )
    };
    let held_back_clause = (!is_filtered_tag).then(|| {
        format!(
            "c.special_state IS NULL AND c.state IN ({learning_states})\n    AND c.due > {learn_ahead_until} AND c.due <= {}{card_id_query_str}",
            card_due_limit.timestamp()
        )
    });
    let caps = if is_filtered_tag {
        QueueCaps::default()
    } else {
        QueueCaps::from_limits(&limits_left)
    };
    Ok(Some(ReviewFilterQuery {
        where_clause,
        is_filtered_tag,
        card_due_limit,
        caps,
        preset_sql,
        held_back_clause,
        query_sort,
    }))
}

/// What is left of a review with `filter` today: how many cards in each state, and how long they
/// should take, from each card's average review time (30 seconds for a card never reviewed).
///
/// New and review cards are capped by `filter.caps`, what is left of today's limits, since the
/// queue shows no more than that. For example, with `reviews_daily_limit = 200`, 600 reviews due
/// after a holiday and 50 already done today, reviews show 150 rather than 550, so the count
/// reaches 0 when the day's reviews are done rather than looking endless. With
/// `new_cards_daily_limit = 20` and 5 new cards done, new shows 15 rather than every new card in
/// the collection. Learning and relearning cards are never held back, so they are never capped,
/// and filtered tags (whose caps are `None`) count every card in them.
///
/// The cards of a [preset](crate::config::QueuePreset) with limits of its own are capped by those
/// too, before the total is (see [`cap_counts`]). With 30 new `languages` cards and 10 left of
/// its limit, and 5 other new cards, new shows 15.
///
/// The time estimate of a capped state is scaled down to the cards counted, by that state's
/// average time per card: 600 reviews taking 5 hours in all, capped to 150, take 1h15.
pub(crate) async fn get_cards_left_by_state_and_time_estimate(
    db: &SqlitePool,
    filter: &ReviewFilterQuery,
) -> Result<(HashMap<StateId, u32>, Duration), Error> {
    let ReviewFilterQuery {
        where_clause,
        is_filtered_tag,
        card_due_limit,
        caps,
        preset_sql,
        ..
    } = filter;
    // These are the remaining cards that are due on `requested_date` grouped by state, with the
    // seconds they should take. Note that this is _not_ equivalent to the cards by state in
    // `get_statistics()` because that does not filter by `requested_date`. The cast is necessary
    // since the SUM function returns a different type depending on whether the arguments are all
    // null or not.
    let query_str = format!(
        indoc! {
        "SELECT
            c.state,
            {} AS preset,
            COUNT(*) as count,
            SUM(CAST(COALESCE(avg_duration, {}) as REAL)) as total_time
        FROM card c
        JOIN note n ON c.note_id = n.id
        JOIN parser p ON n.parser_id = p.id
        LEFT JOIN (
            SELECT card_id, AVG(recall_duration + rate_duration) as avg_duration
            FROM review_log
            WHERE kind = {}
            GROUP BY card_id
        ) rl ON rl.card_id = c.id
        WHERE {}
        GROUP BY 1, 2"
        },
        if *is_filtered_tag { "NULL" } else { preset_sql },
        DEFAULT_ESTIMATED_CARD_REVIEW_SECONDS,
        ReviewLogKind::Review as u8,
        where_clause
    );
    let mut query = sqlx::query_as(&query_str);
    if !is_filtered_tag {
        query = query.bind(card_due_limit.timestamp());
    }
    let rows: Vec<(StateId, Option<i64>, u32, f64)> = query
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    let counted = cap_counts(
        rows.into_iter()
            .map(|(state, preset, count, time)| (state, preset_index(preset), count, time)),
        caps,
    );
    let total_time_seconds: f64 = counted.values().map(|(_, time)| time).sum();
    let cards_left_by_state = counted
        .into_iter()
        .map(|(state, (count, _))| (state, count))
        .collect();
    let time_estimate = Duration::seconds(total_time_seconds as i64);

    Ok((cards_left_by_state, time_estimate))
}

pub(crate) async fn unbury_cards_and_update_config(db: &SqlitePool) -> Result<(), Error> {
    let mut config = read_internal_config(db).await?;
    let now = Utc::now();
    // Compare dates in local timezone to determine if cards should be unburied.
    // This ensures that cards are unburied when the calendar date changes in the user's timezone,
    // regardless of when reviews were performed (e.g., late at night vs. early morning).
    // For example, if a user reviews at 11 PM on day 1 and then at 6 AM on day 2, the cards
    // will be unburied on day 2 even though less than 24 hours have passed.
    let now_local_date = now.with_timezone(&Local).date_naive();
    // Handle the case where last_unburied might be MIN_UTC or out of range for local timezone
    // If it's MIN_UTC, treat it as never unburied, so we should always unbury
    let last_unburied_local_date = if config.last_unburied == DateTime::<Utc>::MIN_UTC {
        // Use a date far in the past that's safe for comparison
        // This ensures we always unbury on the first run
        NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()
    } else {
        config.last_unburied.with_timezone(&Local).date_naive()
    };
    if now_local_date > last_unburied_local_date {
        // Unbury
        unbury_cards(db, None, now, false).await?;

        // Update config
        config.last_unburied = now;
        write_internal_config(db, &config).await?;
    }
    Ok(())
}

// Note that `requested_date` is not in `ReviewOptions` since we don't want the user to be able to edit it. However, for testing purposes, we still want to be able to mimic calling this function on different days, so it is included as an argument.
pub async fn get_review_card(
    db: &SqlitePool,
    body: GetReviewCardRequest,
    requested_date: DateTime<Utc>,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<Option<GetReviewCardResponse>, Error> {
    let GetReviewCardRequest { filter } = body;

    // Unbury cards, if needed
    unbury_cards_and_update_config(db).await?;

    // Build the filter query for matching cards
    let Some(review_filter) =
        build_review_filter_query(db, filter.as_ref(), requested_date, true).await?
    else {
        return Ok(None);
    };
    let (cards_left_by_state, time_estimate) =
        get_cards_left_by_state_and_time_estimate(db, &review_filter).await?;
    let ReviewFilterQuery {
        where_clause,
        is_filtered_tag,
        card_due_limit,
        caps,
        query_sort,
        ..
    } = review_filter;

    let config = read_external_config()?;
    let order_by = queue_order_by(
        &config.queue,
        requested_date,
        &caps.showable(&cards_left_by_state),
        &query_sort,
    );
    let query_str = formatdoc! {"
        SELECT {REVIEW_CARD_COLUMNS}
        FROM card c
        JOIN note n ON c.note_id = n.id
        JOIN parser p ON n.parser_id = p.id
        WHERE {where_clause}
        ORDER BY
            {order_by}
        LIMIT 1"
    };
    info!("{}", query_str);
    let mut query = sqlx::query_as(&query_str);
    if !is_filtered_tag {
        query = query.bind(card_due_limit.timestamp());
    }
    let review_card_opt: Option<ReviewCard> = query
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    if let Some(review_card) = review_card_opt {
        let response = review_card_response(
            db,
            review_card,
            cards_left_by_state,
            time_estimate,
            all_parsers,
        )
        .await?;
        return Ok(Some(response));
    }
    Ok(None)
}

/// When review with `filter` can next show a card, once [`get_review_card`] has nothing to show
/// now: the time the earliest learning or relearning card held back by
/// [`QueueConfig::learn_ahead_limit`](crate::config::QueueConfig::learn_ahead_limit) comes within
/// it. `None` if no such card is due later today, so the review is done for the day.
pub async fn next_review_card_available_at(
    db: &SqlitePool,
    GetReviewCardRequest { filter }: GetReviewCardRequest,
    requested_date: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, Error> {
    let Some(ReviewFilterQuery {
        held_back_clause: Some(held_back_clause),
        ..
    }) = build_review_filter_query(db, filter.as_ref(), requested_date, true).await?
    else {
        return Ok(None);
    };
    let due: Option<i64> = sqlx::query_scalar(&format!(
        "SELECT MIN(c.due) FROM card c WHERE {held_back_clause}"
    ))
    .fetch_one(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let learn_ahead_limit = read_external_config()?.queue.learn_ahead_limit;
    Ok(due
        .and_then(|due| DateTime::from_timestamp(due, 0))
        .map(|due| (due - learn_ahead_limit).max(requested_date)))
}

/// Everything a client needs to show `review_card`.
pub(crate) async fn review_card_response(
    db: &SqlitePool,
    review_card: ReviewCard,
    cards_left_by_state: HashMap<StateId, u32>,
    time_estimate: Duration,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<GetReviewCardResponse, Error> {
    let note_data = if matches!(review_card.card_back_type, BackType::Cli) {
        Some(fetch_note_data_for_cli(db, review_card.note_id).await?)
    } else {
        None
    };
    let linked_notes = get_linked_notes_for_review(db, review_card.note_id, all_parsers).await?;
    let keywords: Vec<String> =
        sqlx::query_scalar(r"SELECT keyword FROM note_keyword WHERE note_id = ?")
            .bind(review_card.note_id)
            .fetch_all(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
    let mut response = build_review_card_response(
        review_card,
        note_data,
        cards_left_by_state,
        time_estimate,
        all_parsers,
        linked_notes,
        keywords,
    )?;
    add_card_extras(db, &mut response, all_parsers).await?;
    Ok(response)
}

/// Fills in the parts of `response` that come from parsing its card's note: the sources to compile
/// in the browser, the answers to type and the audio clips to play.
async fn add_card_extras(
    db: &SqlitePool,
    response: &mut GetReviewCardResponse,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<(), Error> {
    let parser = find_parser(&response.parser_name, all_parsers)?;
    let loaded = load_review_card(db, parser.as_ref(), response).await;
    // Only cards compiled in the browser need their note parsed to be shown at all
    let (request, card) = match loaded {
        Ok(loaded) => loaded,
        Err(e) if !parser.renders_in_browser() => {
            warn!(
                "Note {} card {} could not be parsed for its typed answer and audio: {e}",
                response.note_id.letters(),
                response.card_order
            );
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    add_browser_render_sources(parser.as_ref(), response, &request, &card)?;
    response.typed_answer = typed_answer_info(parser.as_ref(), &card)?;
    response.audio = card_audio(parser.as_ref(), &card);
    Ok(())
}

pub async fn get_review_card_by_id(
    db: &SqlitePool,
    card_id: CardId,
    filter: Option<&GetReviewCardFilterRequest>,
    requested_date: DateTime<Utc>,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<Option<GetReviewCardResponse>, Error> {
    let review_card_opt: Option<ReviewCard> = sqlx::query_as(
        r#"SELECT
            n.id as note_id,
            p.name as parser_name,
            c."order" as card_order,
            c.back_type as card_back_type,
            c.id as card_id,
            c.state as card_state
        FROM card c
        JOIN note n ON c.note_id = n.id
        JOIN parser p ON n.parser_id = p.id
        WHERE c.id = ?"#,
    )
    .bind(card_id)
    .fetch_optional(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    match review_card_opt {
        Some(review_card) => {
            // Compute cards_left_by_state and time_estimate using the same filter as the review session
            let (cards_left_by_state, time_estimate) =
                match build_review_filter_query(db, filter, requested_date, true).await? {
                    Some(review_filter) => {
                        get_cards_left_by_state_and_time_estimate(db, &review_filter).await?
                    }
                    // Tag not found — no cards left to review under this filter
                    None => (HashMap::new(), Duration::zero()),
                };

            let response = review_card_response(
                db,
                review_card,
                cards_left_by_state,
                time_estimate,
                all_parsers,
            )
            .await?;
            Ok(Some(response))
        }
        None => Ok(None),
    }
}

/// Updates the card's scheduler data for `filtered_tag`, returning whether this review finished the
/// card's run through the tag. The caller then removes the card from the tag, once the review that
/// references the tag has been written.
pub fn update_filtered_tag_scheduler_data(
    scheduler: &dyn SrsScheduler,
    filtered_tag: &Tag,
    updated_card: &mut Card,
    rating: RatingId,
    recall_duration: Duration,
    reviewed_at: DateTime<Utc>,
) -> Result<bool, Error> {
    let scheduler_name = scheduler.get_scheduler_name();
    let tag_id_str = filtered_tag.id.to_string();
    assert!(matches!(updated_card.custom_data, Value::Object(_)));
    let custom_data = updated_card.custom_data.as_object().unwrap();
    let filtered_tag_scheduler_data = custom_data
        .get(&tag_id_str)
        .and_then(|x| x.get(scheduler_name));
    let new_filtered_tag_scheduler_data_opt = scheduler.filtered_tag_schedule(
        filtered_tag_scheduler_data,
        updated_card,
        rating,
        reviewed_at,
        recall_duration,
    )?;
    let custom_data = updated_card.custom_data.as_object_mut().unwrap();
    if let Some(new_filtered_tag_scheduler_data) = new_filtered_tag_scheduler_data_opt {
        custom_data
            .entry(tag_id_str)
            .and_modify(|v| {
                if let Some(tag_object) = v.as_object_mut() {
                    tag_object.insert(
                        scheduler_name.to_string(),
                        new_filtered_tag_scheduler_data.clone(),
                    );
                }
            })
            .or_insert_with(|| {
                Value::Object(serde_json::Map::from_iter([(
                    scheduler_name.to_string(),
                    new_filtered_tag_scheduler_data,
                )]))
            });
    } else {
        // Delete scheduler data for the filtered tag
        if let Some(tag_object) = custom_data
            .get_mut(&tag_id_str)
            .and_then(|v| v.as_object_mut())
        {
            tag_object.remove(scheduler_name);
            if tag_object.is_empty() {
                custom_data.remove(&tag_id_str);
            }
        }
        return Ok(true);
    }
    Ok(false)
}

pub async fn rate_card(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    submission: RatingSubmission,
    reviewed_at: DateTime<Utc>,
    log: bool,
) -> Result<Option<i64>, Error> {
    let config = read_external_config()?;
    // Boxed, since the config makes a large future for every caller to carry
    Box::pin(rate_card_with_config(
        db,
        scheduler,
        &config,
        submission,
        reviewed_at,
        log,
    ))
    .await
}

/// [`rate_card`] with `config` rather than the one in `config.toml`.
#[expect(clippy::too_many_lines)]
pub(crate) async fn rate_card_with_config(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    config: &SparesExternalConfig,
    RatingSubmission {
        card_id,
        rating,
        recall_duration,
        rate_duration,
        tag_id,
    }: RatingSubmission,
    reviewed_at: DateTime<Utc>,
    log: bool,
) -> Result<Option<i64>, Error> {
    // Validate input
    let filtered_tag_opt = if let Some(tag_id) = tag_id {
        let tag: Tag = sqlx::query_as(r"SELECT * FROM tag WHERE id = ?")
            .bind(tag_id)
            .fetch_one(db)
            .await
            .map_err(|e| Error::Sqlx { source: e })?;
        if tag.query.is_none() {
            return Err(Error::Library(LibraryError::Tag(
                TagErrorKind::InvalidInput("Supplied tag id is not a filtered tag.".to_string()),
            )));
        }
        Some(tag)
    } else {
        None
    };

    let before_card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    if let Some(filtered_tag) = &filtered_tag_opt
        && !filtered_tag.reschedule
    {
        return rate_card_without_rescheduling(
            db,
            scheduler,
            filtered_tag,
            before_card,
            rating,
            recall_duration,
            rate_duration,
            reviewed_at,
            log,
        )
        .await;
    }
    let card = before_card.clone();
    // With the steps and sibling burying of the card's preset
    // Boxed, since every caller of rating carries its future
    let configs = Box::pin(CardConfigs::load(db, config, Some(&[card_id]))).await?;
    let config = configs.get(card_id);

    // Get review logs for this card
    let mut review_logs = fetch_review_logs_for_replay(db, card_id).await?;

    // Schedule card.
    // After a forget the newest row is the marker, and reviews before it were explicitly
    // discarded. Handing either to the scheduler would make this review look like a continuation
    // of a lapse rather than the first review of a reset card.
    let latest_review_log = effective_review_logs(&review_logs).last().cloned();
    let (mut updated_card, mut new_review_log) = scheduler.schedule(
        config,
        &card,
        latest_review_log,
        rating,
        reviewed_at,
        recall_duration,
        rate_duration,
    )?;
    // Validate scheduler's output
    assert!(matches!(updated_card.custom_data, Value::Object(_)));
    assert!(matches!(new_review_log.custom_data, Value::Object(_)));

    // Place the card among the due dates the scheduler allows.
    review_logs.push(new_review_log.clone());
    updated_card.due = place_due(
        db,
        scheduler,
        config,
        &updated_card,
        &review_logs,
        reviewed_at,
    )
    .await?;
    // Record the interval the card was actually given, not the one the scheduler proposed before
    // placement moved it. Postponing reads this back to measure how overdue a card is.
    new_review_log.scheduled_time = Some((updated_card.due - reviewed_at).num_seconds());

    // Update filtered tag scheduler data
    let finished_tag = match &filtered_tag_opt {
        Some(filtered_tag) => update_filtered_tag_scheduler_data(
            scheduler,
            filtered_tag,
            &mut updated_card,
            rating,
            recall_duration,
            reviewed_at,
        )?
        .then_some(filtered_tag),
        None => None,
    };

    // Add entry to review_log
    let lapses_before = scheduler.lapses(db, card_id).await?;
    let review_log_id = insert_review(db, &new_review_log, tag_id).await?;
    let lapses = scheduler.lapses(db, card_id).await?;
    let leech = lapses > lapses_before && config.leech.acts_on(lapses);

    // Update card with all new properties from updated_card
    // special_state is set to None to clear any burial state (e.g. BuriedUntilLaterToday), unless
    // the card became a leech to suspend.
    let special_state =
        (leech && config.leech.action == LeechAction::Suspend).then_some(SpecialState::Suspended);
    let _update_card_result = sqlx::query(
        r"UPDATE card SET due = ?, stability = ?, difficulty = ?, special_state = ?, state = ?, updated_at = ?, custom_data = ? WHERE id = ?",
    )
    .bind(updated_card.due.timestamp())
    .bind(updated_card.stability)
    .bind(updated_card.difficulty)
    .bind(special_state)
    .bind(updated_card.state)
    .bind(updated_card.updated_at.timestamp())
    .bind(updated_card.custom_data.clone())
    .bind(card.id)
    .execute(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    // Siblings are buried until the day is over, so a review of an earlier day, sent late by an
    // offline client, would bury them a day too long.
    let reviewed_today = reviewed_at.with_timezone(&Local).date_naive()
        >= Utc::now().with_timezone(&Local).date_naive();
    let buried_siblings = if filtered_tag_opt.is_none() && reviewed_today {
        bury_siblings(db, &config.queue, &before_card, reviewed_at).await?
    } else {
        Vec::new()
    };

    // Remove the card from the filtered tag it finished. This waits until the review is written,
    // since deleting an emptied tag first would leave the review referencing a missing tag.
    let left_tag = match finished_tag {
        Some(filtered_tag) => {
            let deleted =
                leave_filtered_tag(db, card_id, filtered_tag.id, filtered_tag.auto_delete).await?;
            Some(FilteredTagExit {
                tag_id: filtered_tag.id,
                deleted_tag: deleted.then(|| filtered_tag.clone()),
            })
        }
        None => None,
    };

    if log {
        let payload = RateCardPayload {
            review_log_id: Some(review_log_id),
            card: UpdateCardPayload {
                card_id,
                order: None,
                back_type: None,
                due: Some(Transition {
                    before: before_card.due,
                    after: updated_card.due,
                }),
                stability: Some(Transition {
                    before: before_card.stability,
                    after: updated_card.stability,
                }),
                difficulty: Some(Transition {
                    before: before_card.difficulty,
                    after: updated_card.difficulty,
                }),
                desired_retention: None,
                special_state: (before_card.special_state != special_state).then_some(Transition {
                    before: before_card.special_state,
                    after: special_state,
                }),
                state: Some(Transition {
                    before: before_card.state,
                    after: updated_card.state,
                }),
                custom_data: (updated_card.custom_data != before_card.custom_data).then_some(
                    Transition {
                        before: before_card.custom_data,
                        after: updated_card.custom_data,
                    },
                ),
            },
            left_tag,
            buried_siblings,
        };
        let event_ids = insert_events(
            db,
            &[(EventType::RateCard, to_value(&payload).unwrap())],
            reviewed_at,
            None,
        )
        .await?;
        let event_id = *event_ids.first().unwrap();
        if leech {
            // Boxed, since updating a note makes a large future for every caller to carry
            Box::pin(tag_leech(
                db,
                &config.leech,
                before_card.note_id,
                reviewed_at,
                true,
                Some(event_id),
            ))
            .await?;
        }
        return Ok(Some(event_id));
    }
    if leech {
        Box::pin(tag_leech(
            db,
            &config.leech,
            before_card.note_id,
            reviewed_at,
            false,
            None,
        ))
        .await?;
    }

    Ok(None)
}

/// Rates `before_card` while studying `filtered_tag`, a tag that does not reschedule
/// ([`Tag::reschedule`]): only the card's progress through the tag changes, and the rating is
/// logged as a [`ReviewLogKind::Cram`].
///
/// The cram row is what makes the rating count in statistics and the review heatmap. Every replay
/// of the card's history (rating, `spares card reschedule`, the optimizer, sync, offline reviews
/// inserted into the history) skips it, so the card keeps its memory state and due date, as if
/// the rating had never happened. The undo event records the row in
/// [`RateCardPayload::review_log_id`], so undo deletes it.
///
/// For example, rating a card Good in an `exam-cram` tag while its next review is in 10 days
/// leaves it due in 10 days, writes a cram with rating Good, `previous_state` the card's state
/// and no `scheduled_time`, and moves the card one Good closer to leaving the tag.
#[expect(clippy::too_many_arguments)]
async fn rate_card_without_rescheduling(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    filtered_tag: &Tag,
    before_card: Card,
    rating: RatingId,
    recall_duration: Duration,
    rate_duration: Duration,
    reviewed_at: DateTime<Utc>,
    log: bool,
) -> Result<Option<i64>, Error> {
    let mut updated_card = before_card.clone();
    let finished = update_filtered_tag_scheduler_data(
        scheduler,
        filtered_tag,
        &mut updated_card,
        rating,
        recall_duration,
        reviewed_at,
    )?;
    // Clears any burial, as rating a card always does
    sqlx::query(
        r"UPDATE card SET special_state = NULL, updated_at = ?, custom_data = ? WHERE id = ?",
    )
    .bind(reviewed_at.timestamp())
    .bind(updated_card.custom_data.clone())
    .bind(before_card.id)
    .execute(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let cram = ReviewLog {
        card_id: Some(before_card.id),
        reviewed_at,
        kind: ReviewLogKind::Cram,
        rating: Some(rating),
        scheduler_name: scheduler.get_scheduler_name().to_string(),
        scheduled_time: None,
        recall_duration: Some(recall_duration.num_seconds()),
        rate_duration: Some(rate_duration.num_seconds()),
        previous_state: before_card.state,
        ..ReviewLog::new()
    };
    let review_log_id = insert_review(db, &cram, Some(filtered_tag.id)).await?;
    // Written after the cram, since deleting an emptied tag first would leave the cram
    // referencing a missing tag.
    let left_tag = if finished {
        let deleted = leave_filtered_tag(
            db,
            before_card.id,
            filtered_tag.id,
            filtered_tag.auto_delete,
        )
        .await?;
        Some(FilteredTagExit {
            tag_id: filtered_tag.id,
            deleted_tag: deleted.then(|| filtered_tag.clone()),
        })
    } else {
        None
    };
    if !log {
        return Ok(None);
    }
    let payload = RateCardPayload {
        review_log_id: Some(review_log_id),
        card: UpdateCardPayload {
            card_id: before_card.id,
            order: None,
            back_type: None,
            due: None,
            stability: None,
            difficulty: None,
            desired_retention: None,
            special_state: before_card.special_state.map(|state| Transition {
                before: Some(state),
                after: None,
            }),
            state: None,
            custom_data: (updated_card.custom_data != before_card.custom_data).then_some(
                Transition {
                    before: before_card.custom_data,
                    after: updated_card.custom_data,
                },
            ),
        },
        left_tag,
        buried_siblings: Vec::new(),
    };
    let event_ids = insert_events(
        db,
        &[(EventType::RateCard, to_value(&payload).unwrap())],
        reviewed_at,
        None,
    )
    .await?;
    Ok(Some(*event_ids.first().unwrap()))
}

/// Adds [`LeechConfig::tag_name`] to the note of a card that just became a leech, recording an
/// undo event if `log`. With `group_with`, the event that made it one (the rating, or the
/// suspension of a card an inserted offline review made a leech), the tag's events join that
/// event's group, so undoing it untags the note too.
pub(crate) async fn tag_leech(
    db: &SqlitePool,
    leech: &LeechConfig,
    note_id: NoteId,
    at: DateTime<Utc>,
    log: bool,
    group_with: Option<i64>,
) -> Result<(), Error> {
    let response = update_notes(
        db,
        UpdateNotesRequest {
            selector: NotesSelector::Ids(vec![note_id]),
            parser_id: None,
            data: None,
            keywords: None,
            tags: UpdateTags::ModifyTags {
                tags_to_remove: None,
                tags_to_add: Some(vec![leech.tag_name.clone()]),
            },
            custom_data: None,
        },
        at,
        &get_all_parsers(),
        log,
    )
    .await?;
    if let (Some(leader), Some(tag_event)) = (group_with, response.event_id) {
        join_event_group(db, leader, tag_event).await?;
    }
    Ok(())
}

/// Buries the siblings of `card` (the other cards of its note) that
/// [`QueueConfig::bury_new_siblings`] and [`QueueConfig::bury_review_siblings`] say to, now that it
/// was rated at `reviewed_at`. Returns their changes, for the rating's undo event.
async fn bury_siblings(
    db: &SqlitePool,
    queue: &QueueConfig,
    card: &Card,
    reviewed_at: DateTime<Utc>,
) -> Result<Vec<UpdateCardPayload>, Error> {
    let states = [
        (queue.bury_new_siblings, NEW_CARD_STATE),
        (queue.bury_review_siblings, review_state()),
    ]
    .into_iter()
    .filter_map(|(bury, state)| bury.then_some(state))
    .join(", ");
    if states.is_empty() {
        return Ok(Vec::new());
    }
    let (_, end_of_day) = get_start_end_local_date(&reviewed_at);
    let sibling_ids: Vec<CardId> = sqlx::query_scalar(&format!(
        r"UPDATE card SET special_state = ?, updated_at = ?
        WHERE note_id = ? AND id != ? AND special_state IS NULL AND due <= ? AND state IN ({states})
        RETURNING id"
    ))
    .bind(SpecialState::SchedulerBuried)
    .bind(reviewed_at.timestamp())
    .bind(card.note_id)
    .bind(card.id)
    .bind(end_of_day.timestamp())
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    Ok(sibling_ids
        .into_iter()
        .map(|card_id| UpdateCardPayload {
            card_id,
            order: None,
            back_type: None,
            due: None,
            stability: None,
            difficulty: None,
            desired_retention: None,
            special_state: Some(Transition {
                before: None,
                after: Some(SpecialState::SchedulerBuried),
            }),
            state: None,
            custom_data: None,
        })
        .collect())
}

/// Removes the card from a filtered tag it finished, deleting the tag if `auto_delete` is set and
/// the card was its last. Returns whether the tag was deleted.
pub(crate) async fn leave_filtered_tag(
    db: &SqlitePool,
    card_id: CardId,
    tag_id: TagId,
    auto_delete: bool,
) -> Result<bool, Error> {
    delete_card_tags(db, &[(card_id, tag_id)]).await?;
    if !auto_delete {
        return Ok(false);
    }
    delete_empty_tags(db, &[tag_id]).await?;
    let remaining: i64 = sqlx::query_scalar(r"SELECT COUNT(*) FROM tag WHERE id = ?")
        .bind(tag_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(remaining == 0)
}

/// Reverses [`leave_filtered_tag`] for an undone review, recreating the tag if it was deleted.
pub(crate) async fn rejoin_filtered_tag(
    db: &SqlitePool,
    card_id: CardId,
    left_tag: &FilteredTagExit,
) -> Result<(), Error> {
    if let Some(tag) = &left_tag.deleted_tag {
        // The tag is restored as it was, without rerunning its query, which today could match
        // cards that were never in it.
        sqlx::query(
            r"INSERT OR IGNORE INTO tag (id, name, description, query, auto_delete, reschedule, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(tag.id)
        .bind(&tag.name)
        .bind(&tag.description)
        .bind(&tag.query)
        .bind(tag.auto_delete)
        .bind(tag.reschedule)
        .bind(tag.created_at.timestamp())
        .bind(tag.updated_at.timestamp())
        .execute(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    }
    // Skipped if the tag has since been deleted some other way.
    sqlx::query(
        r"INSERT OR IGNORE INTO card_tag (card_id, tag_id) SELECT ?, id FROM tag WHERE id = ?",
    )
    .bind(card_id)
    .bind(left_tag.tag_id)
    .execute(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    Ok(())
}

/// Adds `review_log` to the review log: a review produced by [`SrsScheduler::schedule`], or a
/// cram built by [`rate_card_without_rescheduling`]. Its `id` and `tag_id` are ignored.
pub(crate) async fn insert_review(
    db: &SqlitePool,
    review_log: &ReviewLog,
    tag_id: Option<TagId>,
) -> Result<i64, Error> {
    let id = next_id_in(db, IdTable::ReviewLog).await?;
    sqlx::query_scalar(r"INSERT INTO review_log (id, card_id, reviewed_at, kind, rating, scheduler_name, scheduled_time, recall_duration, rate_duration, previous_state, tag_id, custom_data) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id")
        .bind(id)
        .bind(review_log.card_id)
        .bind(review_log.reviewed_at.timestamp())
        .bind(review_log.kind)
        .bind(review_log.rating)
        .bind(&review_log.scheduler_name)
        .bind(review_log.scheduled_time)
        .bind(review_log.recall_duration)
        .bind(review_log.rate_duration)
        .bind(review_log.previous_state)
        // Records which filtered tag the card was answered under, if any. The scheduler has
        // no notion of tags, so this is stamped here rather than in `schedule`.
        .bind(tag_id)
        .bind(&review_log.custom_data)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })
}

pub async fn bury_card(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    card_id: CardId,
    at: DateTime<Utc>,
    log: bool,
) -> Result<Option<i64>, Error> {
    let before_card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
        .bind(card_id)
        .fetch_one(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;

    validate_bury_target(before_card.special_state)?;

    let buried_card = scheduler.bury(&before_card)?;

    // Update card with all new properties from buried_card
    let _update_card_result = sqlx::query(
        r"UPDATE card SET due = ?, stability = ?, difficulty = ?, special_state = ?, state = ?, updated_at = ? WHERE id = ?",
    )
    .bind(buried_card.due.timestamp())
    .bind(buried_card.stability)
    .bind(buried_card.difficulty)
    .bind(buried_card.special_state)
    .bind(buried_card.state)
    .bind(at.timestamp())
    .bind(card_id)
    .execute(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;

    if log {
        let payload = vec![UpdateCardPayload {
            card_id,
            order: None,
            back_type: None,
            due: (buried_card.due != before_card.due).then_some(Transition {
                before: before_card.due,
                after: buried_card.due,
            }),
            stability: ((buried_card.stability - before_card.stability).abs() > ALLOWED_F64_ERROR)
                .then_some(Transition {
                    before: before_card.stability,
                    after: buried_card.stability,
                }),
            difficulty: ((buried_card.difficulty - before_card.difficulty).abs()
                > ALLOWED_F64_ERROR)
                .then_some(Transition {
                    before: before_card.difficulty,
                    after: buried_card.difficulty,
                }),
            desired_retention: None,
            special_state: Some(Transition {
                before: before_card.special_state,
                after: buried_card.special_state,
            }),
            state: (buried_card.state != before_card.state).then_some(Transition {
                before: before_card.state,
                after: buried_card.state,
            }),
            custom_data: None,
        }];
        let event_ids = insert_events(
            db,
            &[(EventType::BuryCards, to_value(&payload).unwrap())],
            at,
            None,
        )
        .await?;
        return Ok(Some(*event_ids.first().unwrap()));
    }
    Ok(None)
}

// Note that `reviewed_at` is not present in the request body since we don't want the user to be able to edit it. However, for testing purposes, we still want to be able to mimic calling this function on different days, so it is included as an argument.
pub async fn submit_study_action(
    db: &SqlitePool,
    body: SubmitStudyActionRequest,
    at: DateTime<Utc>,
) -> Result<SubmitStudyActionResponse, Error> {
    let SubmitStudyActionRequest {
        scheduler_name,
        action,
    } = body;

    let scheduler = get_scheduler_from_string(scheduler_name.as_str())?;

    match action {
        StudyAction::Rate(rating_submission) => {
            return rate_card(db, scheduler.as_ref(), rating_submission, at, true)
                .await
                .map(|event_id| SubmitStudyActionResponse { event_id });
        }
        StudyAction::Bury { card_id } => {
            return bury_card(db, scheduler.as_ref(), card_id, at, true)
                .await
                .map(|event_id| SubmitStudyActionResponse { event_id });
        }
        StudyAction::Advance { count, query } => {
            let config = read_external_config()?;
            let move_cards_result = scheduler.advance(db, &config, count, query, at).await?;
            if !move_cards_result.card_payloads.is_empty() {
                insert_events(
                    db,
                    &[(
                        EventType::AdvanceCards,
                        to_value(&move_cards_result.card_payloads).unwrap(),
                    )],
                    at,
                    None,
                )
                .await?;
            }
        }
        StudyAction::Postpone { count, query } => {
            let config = read_external_config()?;
            let move_cards_result = scheduler.postpone(db, &config, count, query, at).await?;
            if !move_cards_result.card_payloads.is_empty() {
                insert_events(
                    db,
                    &[(
                        EventType::PostponeCards,
                        to_value(&move_cards_result.card_payloads).unwrap(),
                    )],
                    at,
                    None,
                )
                .await?;
            }
        }
    }
    Ok(SubmitStudyActionResponse { event_id: None })
}

/// Recomputes every card's memory state from its review log and gives it a new due date, with
/// the steps of each card's [preset](crate::config::QueuePreset).
pub(crate) async fn reschedule_all_cards(
    db: &SqlitePool,
    scheduler: &dyn SrsScheduler,
    config: &SparesExternalConfig,
    at: DateTime<Utc>,
) -> Result<(), Error> {
    // Suspended and buried cards are included: they keep their special state, but should come
    // back with a memory state and due date that match their history.
    let cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card")
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    // Get all review logs for cards. Every card is a candidate here, so this fetches the whole
    // table rather than building an `IN` list. Orphaned rows (deleted cards) are excluded up front
    // rather than in Rust: there is nothing to reschedule for them, and on a long-lived database
    // they only grow in number. Forget markers are kept for the cards that remain:
    // `compute_memory_state` needs them to know where to restart replay. Crams are left out, like
    // `fetch_review_logs_for_replay` does, so placement does not measure intervals from them.
    let all_review_logs: Vec<ReviewLog> = sqlx::query_as(
        r"SELECT * FROM review_log WHERE card_id IS NOT NULL AND kind < ?
          ORDER BY card_id, reviewed_at ASC, id ASC",
    )
    .bind(ReviewLogKind::FIRST_UNREPLAYED)
    .fetch_all(db)
    .await
    .map_err(|e| Error::Sqlx { source: e })?;
    let grouped_review_logs = all_review_logs
        .into_iter()
        .filter_map(|rl| rl.card_id.map(|card_id| (card_id, rl)))
        .into_group_map();
    let configs = CardConfigs::load(db, config, None).await?;
    let cards_with_review_logs = cards
        .into_iter()
        .map(|card| {
            let review_logs = grouped_review_logs
                .get(&card.id)
                .cloned()
                .unwrap_or_default();
            (card, review_logs)
        })
        .collect::<Vec<_>>();
    scheduler
        .reschedule(db, &configs, cards_with_review_logs, at)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::note::tests::tests::create_note_helper;
    use crate::api::note::update_notes;
    use crate::api::statistics::get_statistics;
    use crate::api::tag::create_tag;
    use crate::model::Card;
    use crate::model::TagId;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::NoteResponse;
    use crate::schema::note::NotesSelector;
    use crate::schema::note::UpdateNotesRequest;
    use crate::schema::note::UpdateTags;
    use crate::schema::review::StatisticsRequest;

    #[test]
    fn paths_relative_to_files_dir_resolve_back() {
        let inside = get_files_dir().join("markdown").join("0001.md");
        let relative = relative_to_files_dir(inside.clone());
        assert_eq!(relative, PathBuf::from("markdown/0001.md"));
        assert_eq!(crate::config::resolve_in_files_dir(&relative), inside);

        let outside = PathBuf::from("/elsewhere/0001.md");
        assert_eq!(relative_to_files_dir(outside.clone()), outside);
        assert_eq!(crate::config::resolve_in_files_dir(&outside), outside);
    }

    async fn create_note(pool: &sqlx::SqlitePool) -> (NoteResponse, Vec<Card>) {
        // Create note
        let created_notes = create_note_helper(pool).await;
        let last_note = created_notes.last().unwrap();

        // Get card_id for note
        let cards_res: Result<Vec<Card>, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM card WHERE note_id = ? ORDER BY due")
                .bind(last_note.id)
                .fetch_all(pool)
                .await;
        assert!(cards_res.is_ok());
        let cards = cards_res.unwrap();
        assert!(!cards.is_empty());

        (last_note.clone(), cards)
    }

    #[sqlx::test]
    async fn test_review_card_typed_answer(pool: SqlitePool) -> () {
        use crate::api::note::create_notes;
        use crate::api::parser::tests::create_parser_helper;
        use crate::parsers::TypeAnswer;
        use crate::parsers::typed_answer::TypedAnswerInfo;
        use crate::parsers::typed_answer::TypedBlank;
        use crate::schema::note::CreateNoteRequest;
        use crate::schema::note::CreateNotesRequest;

        let parser = create_parser_helper(&pool, "markdown").await;
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: "The capital of France is {{[t:ci;h:city] *Paris*}}. {{Not typed}}"
                    .to_string(),
                keywords: Vec::new(),
                tags: Vec::new(),
                is_suspended: false,
                custom_data: serde_json::Map::new(),
            }],
        };
        let now = Utc::now();
        create_notes(&pool, request, now, &get_all_parsers(), false)
            .await
            .unwrap();
        let card_ids: Vec<CardId> = sqlx::query_scalar(r"SELECT id FROM card ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        let mut typed_answers = Vec::new();
        for card_id in card_ids {
            let review_card = get_review_card_by_id(&pool, card_id, None, now, &get_all_parsers())
                .await
                .unwrap()
                .unwrap();
            typed_answers.push((review_card.card_order, review_card.typed_answer));
        }
        typed_answers.sort_by_key(|(order, _)| *order);
        assert_eq!(
            typed_answers,
            vec![
                (
                    1,
                    Some(TypedAnswerInfo {
                        mode: TypeAnswer::Diff {
                            ignore_case: true,
                            ignore_combining: false,
                        },
                        blanks: vec![TypedBlank {
                            expected: "Paris".to_string(),
                            hint: Some("city".to_string()),
                        }],
                    })
                ),
                (2, None),
            ]
        );
    }

    #[sqlx::test]
    async fn test_review_card_audio(pool: SqlitePool) -> () {
        use crate::api::note::create_notes;
        use crate::api::parser::tests::create_parser_helper;
        use crate::parsers::audio::CardAudio;
        use crate::schema::note::CreateNoteRequest;
        use crate::schema::note::CreateNotesRequest;

        let parser = create_parser_helper(&pool, "markdown").await;
        let request = CreateNotesRequest {
            parser_id: parser.id,
            requests: vec![CreateNoteRequest {
                data: "[🔊](/m/q.mp3) Translate: {{ [🔊](/m/a.mp3) good morning}}".to_string(),
                keywords: Vec::new(),
                tags: Vec::new(),
                is_suspended: false,
                custom_data: serde_json::Map::new(),
            }],
        };
        let now = Utc::now();
        create_notes(&pool, request, now, &get_all_parsers(), false)
            .await
            .unwrap();
        let card_id: CardId = sqlx::query_scalar(r"SELECT id FROM card")
            .fetch_one(&pool)
            .await
            .unwrap();
        let review_card = get_review_card_by_id(&pool, card_id, None, now, &get_all_parsers())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            review_card.audio,
            Some(CardAudio {
                front: vec!["/m/q.mp3".to_string()],
                back: vec!["/m/a.mp3".to_string()],
            })
        );
    }

    #[sqlx::test]
    async fn test_get_and_update_review(pool: sqlx::SqlitePool) -> () {
        // Create note
        let (_note, _cards) = create_note(&pool).await;
        let now = Utc::now();

        // Get review
        let review_res = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await;
        assert!(review_res.is_ok());
        let review_card_opt = review_res.unwrap();
        assert!(review_card_opt.is_some());
        let review_card = review_card_opt.unwrap();
        // These assertions can't be made because all notes are created at the same time, so the cards are also created at the same time, so they are due at the same time. Thus, the order of cards being due is not guaranteed to be the same.
        // assert_eq!(review_card.note_id, note_id);
        // assert_eq!(review_card.card_id, card_id);
        // assert_eq!(
        //     review_card
        //         .card_rendered_path
        //         .file_stem()
        //         .unwrap()
        //         .to_str()
        //         .unwrap(),
        //     format!("{:0>4}-{:0>1}", note_id, 1)
        // );
        assert_eq!(review_card.parser_name, "markdown".to_string());

        // Get old card
        let old_card_res: Result<Card, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
                .bind(review_card.card_id)
                .fetch_one(&pool)
                .await;
        assert!(old_card_res.is_ok());
        let old_card = old_card_res.unwrap();

        // Get statistics
        let request = StatisticsRequest {
            scheduler_name: "fsrs".to_string(),
            date: now,
            tz: None,
        };
        let statistics_res = get_statistics(&pool, request).await;
        assert!(statistics_res.is_ok());
        let statistics_response = statistics_res.unwrap();
        assert_eq!(
            statistics_response.due_count_by_state.get(&NEW_CARD_STATE),
            Some(&3)
        );
        assert_eq!(
            statistics_response.due_count_by_state.values().sum::<u32>(),
            3
        );
        assert_eq!(statistics_response.advance_safe_count, 0);
        assert_eq!(statistics_response.postpone_safe_count, 0);

        // Update review
        let request = SubmitStudyActionRequest {
            scheduler_name: "fsrs".to_string(),
            action: StudyAction::Rate(RatingSubmission {
                card_id: review_card.card_id,
                rating: 4,
                recall_duration: Duration::seconds(5),
                rate_duration: Duration::seconds(5),
                tag_id: None,
            }),
        };
        let submit_review_res = submit_study_action(&pool, request, now).await;
        assert!(submit_review_res.is_ok());

        // Check database and verify card is now due later
        let new_card_res: Result<Card, sqlx::Error> =
            sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
                .bind(review_card.card_id)
                .fetch_one(&pool)
                .await;
        assert!(new_card_res.is_ok());
        let new_card = new_card_res.unwrap();
        assert!(new_card.due > old_card.due);
    }

    #[sqlx::test]
    async fn test_bury_until_later_today_hides_card_while_others_remain(
        pool: sqlx::SqlitePool,
    ) -> () {
        // create_note_helper creates 3 notes, 1 card each
        let _ = create_note(&pool).await;
        let all_cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card ORDER BY id ASC")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(all_cards.len(), 3);

        let now = Utc::now();

        // Initialize last_unburied = today so subsequent get_review_card calls don't unbury
        let _ = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap();

        // Bury card_a as BuriedUntilLaterToday
        let card_a_id = all_cards[0].id;
        crate::api::card::update_cards(
            &pool,
            crate::schema::card::UpdateCardsRequest {
                selector: crate::schema::card::CardsSelector::Ids(vec![card_a_id]),
                desired_retention: None,
                special_state: Some(Some(
                    crate::schema::card::SpecialStateUpdate::BuriedUntilLaterToday,
                )),
                due: Some(now),
            },
            now,
            false,
        )
        .await
        .unwrap();

        // Next review card should NOT be card_a (it's buried, 2 normal cards remain)
        let review = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_ne!(
            review.card_id, card_a_id,
            "BuriedUntilLaterToday card should not be shown while normal cards remain"
        );

        // Confirm the card is still in BuriedUntilLaterToday state
        let card_a: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
            .bind(card_a_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            card_a.special_state,
            Some(SpecialState::BuriedUntilLaterToday)
        );
    }

    #[sqlx::test]
    async fn test_bury_until_later_today_reappears_when_no_normal_cards(
        pool: sqlx::SqlitePool,
    ) -> () {
        // create_note_helper creates 3 notes, 1 card each
        let _ = create_note(&pool).await;
        let all_cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card ORDER BY id ASC")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(all_cards.len(), 3);

        let now = Utc::now();

        // Initialize last_unburied = today
        let _ = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap();

        // Bury card_a (index 0) as BuriedUntilLaterToday
        let card_a_id = all_cards[0].id;
        crate::api::card::update_cards(
            &pool,
            crate::schema::card::UpdateCardsRequest {
                selector: crate::schema::card::CardsSelector::Ids(vec![card_a_id]),
                desired_retention: None,
                special_state: Some(Some(
                    crate::schema::card::SpecialStateUpdate::BuriedUntilLaterToday,
                )),
                due: Some(now),
            },
            now,
            false,
        )
        .await
        .unwrap();

        // Rate the remaining 2 normal cards (rating=4 schedules them far in the future)
        let scheduler = get_scheduler_from_string("fsrs").unwrap();
        for card in &all_cards[1..] {
            rate_card(
                &pool,
                scheduler.as_ref(),
                crate::schema::review::RatingSubmission {
                    card_id: card.id,
                    rating: 4,
                    recall_duration: Duration::seconds(5),
                    rate_duration: Duration::seconds(2),
                    tag_id: None,
                },
                now,
                false,
            )
            .await
            .unwrap();
        }

        // card_a (BuriedUntilLaterToday) should now be the only reviewable card
        let review = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap();
        assert!(
            review.is_some(),
            "Expected BuriedUntilLaterToday card to reappear when no normal cards remain"
        );
        assert_eq!(
            review.unwrap().card_id,
            card_a_id,
            "Expected card_a to be returned"
        );
    }

    #[sqlx::test]
    async fn test_bury_until_later_today_fifo_ordering(pool: sqlx::SqlitePool) -> () {
        // create_note_helper creates 3 notes, 1 card each
        let _ = create_note(&pool).await;
        let all_cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card ORDER BY id ASC")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(all_cards.len(), 3);

        let now = Utc::now();

        // Initialize last_unburied = today
        let _ = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap();

        let card_a_id = all_cards[0].id;
        let card_b_id = all_cards[1].id;
        let card_c_id = all_cards[2].id;

        // Bury all 3 cards with increasing timestamps (t1 < t2 < t3)
        let t1 = now;
        let t2 = now + Duration::seconds(1);
        let t3 = now + Duration::seconds(2);

        for (id, t) in [(card_a_id, t1), (card_b_id, t2), (card_c_id, t3)] {
            crate::api::card::update_cards(
                &pool,
                crate::schema::card::UpdateCardsRequest {
                    selector: crate::schema::card::CardsSelector::Ids(vec![id]),
                    desired_retention: None,
                    special_state: Some(Some(
                        crate::schema::card::SpecialStateUpdate::BuriedUntilLaterToday,
                    )),
                    due: Some(t),
                },
                now,
                false,
            )
            .await
            .unwrap();
        }

        // First review should be card_a (earliest burial timestamp t1)
        let review1 = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            review1.card_id, card_a_id,
            "card_a (earliest burial) should come first"
        );

        // Re-bury card_a with a later timestamp (t4 > t3), pushing it to the back
        let t4 = now + Duration::seconds(3);
        crate::api::card::update_cards(
            &pool,
            crate::schema::card::UpdateCardsRequest {
                selector: crate::schema::card::CardsSelector::Ids(vec![card_a_id]),
                desired_retention: None,
                special_state: Some(Some(
                    crate::schema::card::SpecialStateUpdate::BuriedUntilLaterToday,
                )),
                due: Some(t4),
            },
            now,
            false,
        )
        .await
        .unwrap();

        // Now card_b (t2) should be first, then card_c (t3), then card_a (t4)
        let review2 = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            review2.card_id, card_b_id,
            "card_b should be shown after card_a is re-buried to the back"
        );
    }

    #[sqlx::test]
    async fn test_bury_until_later_today_unburied_next_day(pool: sqlx::SqlitePool) -> () {
        // create_note_helper creates 3 notes, 1 card each
        let _ = create_note(&pool).await;
        let all_cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card ORDER BY id ASC")
            .fetch_all(&pool)
            .await
            .unwrap();

        let now = Utc::now();

        // Initialize last_unburied = today to prevent immediate unburying
        let _ = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap();

        // Bury all cards as BuriedUntilLaterToday
        for card in &all_cards {
            crate::api::card::update_cards(
                &pool,
                crate::schema::card::UpdateCardsRequest {
                    selector: crate::schema::card::CardsSelector::Ids(vec![card.id]),
                    desired_retention: None,
                    special_state: Some(Some(
                        crate::schema::card::SpecialStateUpdate::BuriedUntilLaterToday,
                    )),
                    due: Some(now),
                },
                now,
                false,
            )
            .await
            .unwrap();
        }

        // Verify all cards are buried
        let buried_count: i64 =
            sqlx::query_scalar(r"SELECT COUNT(*) FROM card WHERE special_state = ?")
                .bind(SpecialState::BuriedUntilLaterToday)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(buried_count, 3);

        // Simulate next day by calling unbury_cards directly
        let tomorrow = now + Duration::days(1);
        crate::api::card::unbury_cards(&pool, None, tomorrow, false)
            .await
            .unwrap();

        // All cards should now have special_state = NULL
        let still_buried_count: i64 =
            sqlx::query_scalar(r"SELECT COUNT(*) FROM card WHERE special_state = ?")
                .bind(SpecialState::BuriedUntilLaterToday)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            still_buried_count, 0,
            "All BuriedUntilLaterToday cards should be unburied at next-day rollover"
        );
    }

    #[sqlx::test]
    async fn test_get_review_card_by_id_respects_filter(pool: sqlx::SqlitePool) -> () {
        let now = Utc::now();

        // Create notes (3 notes from helper)
        let created_notes = create_note_helper(&pool).await;
        assert_eq!(created_notes.len(), 3);

        // Get all cards (1 per note)
        let all_cards: Vec<Card> = sqlx::query_as(r"SELECT * FROM card ORDER BY id ASC")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(all_cards.len(), 3);

        // Initialize last_unburied = today so subsequent calls don't unbury
        let _ = get_review_card(
            &pool,
            GetReviewCardRequest { filter: None },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap();

        // Test with filter=None -> should count all 3 cards
        let response = get_review_card_by_id(&pool, all_cards[0].id, None, now, &get_all_parsers())
            .await
            .unwrap()
            .unwrap();
        let total_all: u32 = response.cards_left_by_state.values().sum();
        assert_eq!(total_all, 3);

        // Create a filtered tag whose query matches cards tagged "tag 1"
        // (first two notes have "tag 1", third has none)
        let tag_request = CreateTagRequest {
            name: "test-by-id-filter".to_string(),
            description: String::new(),
            query: Some(r#"tag="tag 1""#.to_string()),
            auto_delete: false,
            reschedule: true,
        };
        let tag = create_tag(&pool, tag_request, false).await.unwrap();

        // Test with FilteredTag filter -> should count only the 2 cards matching "tag 1"
        let response = get_review_card_by_id(
            &pool,
            all_cards[0].id,
            Some(&GetReviewCardFilterRequest::FilteredTag { tag_id: tag.id }),
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap()
        .unwrap();
        let total_filtered: u32 = response.cards_left_by_state.values().sum();
        assert_eq!(total_filtered, 2);
    }

    /// Three cards, one per note from `create_note_helper`:
    /// - `due`: new, due an hour ago, tagged `tag 1`
    /// - `review`: in review state, due an hour ago, tagged `tag 1`
    /// - `suspended`: new, due a day ago (so it sorts first), suspended, untagged
    ///
    /// An unscoped `c.state=0 limit=1` picks `suspended`; a review snapshot must pick `due`.
    async fn setup_snapshot_cards(
        pool: &SqlitePool,
        now: DateTime<Utc>,
    ) -> (CardId, CardId, CardId) {
        create_note_helper(pool).await;
        let card_ids: Vec<CardId> = sqlx::query_scalar(r"SELECT id FROM card ORDER BY id ASC")
            .fetch_all(pool)
            .await
            .unwrap();
        let [due, review, suspended] = card_ids[..] else {
            panic!("expected 3 cards, got {}", card_ids.len());
        };
        let hour_ago = (now - Duration::hours(1)).timestamp();
        let day_ago = (now - Duration::days(1)).timestamp();
        for (card_id, state, due_at, special_state) in [
            (due, NEW_CARD_STATE, hour_ago, None),
            (review, 2, hour_ago, None),
            (
                suspended,
                NEW_CARD_STATE,
                day_ago,
                Some(SpecialState::Suspended as u8),
            ),
        ] {
            sqlx::query(r"UPDATE card SET state = ?, due = ?, special_state = ? WHERE id = ?")
                .bind(state)
                .bind(due_at)
                .bind(special_state)
                .bind(card_id)
                .execute(pool)
                .await
                .unwrap();
        }
        (due, review, suspended)
    }

    async fn snapshot(pool: &SqlitePool, query: &str, at: DateTime<Utc>) -> ReviewSnapshotResponse {
        create_review_snapshot_tag(
            pool,
            ReviewSnapshotRequest {
                query: query.to_string(),
            },
            at,
        )
        .await
        .unwrap()
    }

    #[sqlx::test]
    async fn test_rate_card_deletes_emptied_auto_delete_filtered_tag(pool: SqlitePool) -> () {
        let created_notes = create_note_helper(&pool).await;
        let card: Card = sqlx::query_as(r"SELECT * FROM card WHERE note_id = ?")
            .bind(created_notes[2].id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let tag = create_tag(
            &pool,
            CreateTagRequest {
                name: "auto-delete filtered tag".to_string(),
                description: String::new(),
                query: Some(format!("c.id={}", card.id)),
                auto_delete: true,
                reschedule: true,
            },
            false,
        )
        .await
        .unwrap();
        assert_eq!(tag_card_ids(&pool, tag.id).await, vec![card.id]);

        // An Easy rating finishes the only card in the tag.
        let scheduler = get_scheduler_from_string("fsrs").unwrap();
        let now = Utc::now();
        rate_card(
            &pool,
            scheduler.as_ref(),
            RatingSubmission {
                card_id: card.id,
                rating: 4,
                recall_duration: Duration::seconds(5),
                rate_duration: Duration::seconds(2),
                tag_id: Some(tag.id),
            },
            now,
            false,
        )
        .await
        .unwrap();

        let tag_count: i64 = sqlx::query_scalar(r"SELECT COUNT(*) FROM tag WHERE id = ?")
            .bind(tag.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(tag_count, 0, "the emptied auto-delete tag must be deleted");
        let review_count: i64 =
            sqlx::query_scalar(r"SELECT COUNT(*) FROM review_log WHERE card_id = ?")
                .bind(card.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(review_count, 1, "the review must still be recorded");
    }

    async fn tag_card_ids(pool: &SqlitePool, tag_id: TagId) -> Vec<CardId> {
        sqlx::query_scalar(r"SELECT card_id FROM card_tag WHERE tag_id = ? ORDER BY card_id")
            .bind(tag_id)
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[sqlx::test]
    async fn test_review_snapshot_limits_count_due_cards_only(pool: SqlitePool) -> () {
        let now = Utc::now();
        let (due, review, _suspended) = setup_snapshot_cards(&pool, now).await;

        let response = snapshot(&pool, "(c.state=0 limit=1) or (c.state=2 limit=5)", now).await;
        assert!(response.rebuilt);
        assert_eq!(response.card_count, 2);
        assert_eq!(
            tag_card_ids(&pool, response.tag_id).await,
            vec![due, review]
        );

        // The snapshot is reviewed through the regular filtered-tag path.
        let review_card = get_review_card(
            &pool,
            GetReviewCardRequest {
                filter: Some(GetReviewCardFilterRequest::FilteredTag {
                    tag_id: response.tag_id,
                }),
            },
            now,
            &get_all_parsers(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(review_card.cards_left_by_state.values().sum::<u32>(), 2);
    }

    #[sqlx::test]
    async fn test_review_snapshot_resumes_same_day_and_rebuilds_next_day(pool: SqlitePool) -> () {
        let now = Utc::now();
        let (due, review, _suspended) = setup_snapshot_cards(&pool, now).await;
        let query = "(c.state=0 limit=1) or (c.state=2 limit=5)";

        let first = snapshot(&pool, query, now).await;
        assert!(first.rebuilt);

        // Whitespace differences map to the same snapshot, which is resumed unchanged.
        let resumed = snapshot(&pool, "(c.state=0  limit=1)  or (c.state=2 limit=5)", now).await;
        assert_eq!(resumed.tag_id, first.tag_id);
        assert_eq!(resumed.tag_name, first.tag_name);
        assert!(!resumed.rebuilt);
        assert_eq!(tag_card_ids(&pool, first.tag_id).await, vec![due, review]);

        // Finishing today's set must not make room for more cards today.
        sqlx::query(r"DELETE FROM card_tag WHERE tag_id = ?")
            .bind(first.tag_id)
            .execute(&pool)
            .await
            .unwrap();
        let finished = snapshot(&pool, query, now).await;
        assert!(!finished.rebuilt);
        assert_eq!(finished.card_count, 0);

        // The next day, the tag is rebuilt from what is due then, and stale filtered-tag
        // scheduler data is dropped.
        let tag_key = first.tag_id.to_string();
        sqlx::query(r"UPDATE card SET custom_data = json_set(custom_data, '$.' || json_quote(?), json('{}')) WHERE id = ?")
            .bind(&tag_key)
            .bind(review)
            .execute(&pool)
            .await
            .unwrap();
        let tomorrow = now + Duration::days(1);
        let next_day = snapshot(&pool, query, tomorrow).await;
        assert_eq!(next_day.tag_id, first.tag_id);
        assert!(next_day.rebuilt);
        assert_eq!(tag_card_ids(&pool, first.tag_id).await, vec![due, review]);
        let tag: Tag = sqlx::query_as(r"SELECT * FROM tag WHERE id = ?")
            .bind(first.tag_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(tag.updated_at.timestamp(), tomorrow.timestamp());
        let review_card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
            .bind(review)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(review_card.custom_data.get(&tag_key).is_none());
    }

    #[sqlx::test]
    async fn test_review_snapshot_is_not_reshuffled_by_note_edits(pool: SqlitePool) -> () {
        let now = Utc::now();
        let (due, _review, suspended) = setup_snapshot_cards(&pool, now).await;

        let response = snapshot(&pool, "(c.state=0 limit=1) or (c.state=2 limit=0)", now).await;
        assert_eq!(tag_card_ids(&pool, response.tag_id).await, vec![due]);
        let tag_before: Tag = sqlx::query_as(r"SELECT * FROM tag WHERE id = ?")
            .bind(response.tag_id)
            .fetch_one(&pool)
            .await
            .unwrap();

        // Unscoped, `c.state=0 limit=1` picks the suspended card, so re-evaluating the query on
        // this edit would pull it into the tag. Note edits must never rebuild filtered tags.
        let suspended_note_id: NoteId =
            sqlx::query_scalar(r"SELECT note_id FROM card WHERE id = ?")
                .bind(suspended)
                .fetch_one(&pool)
                .await
                .unwrap();
        update_notes(
            &pool,
            UpdateNotesRequest {
                selector: NotesSelector::Ids(vec![suspended_note_id]),
                parser_id: None,
                data: None,
                keywords: Some(vec!["edited".to_string()]),
                tags: UpdateTags::None,
                custom_data: None,
            },
            now,
            &get_all_parsers(),
            false,
        )
        .await
        .unwrap();

        assert_eq!(tag_card_ids(&pool, response.tag_id).await, vec![due]);
        let tag_after: Tag = sqlx::query_as(r"SELECT * FROM tag WHERE id = ?")
            .bind(response.tag_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(tag_after.updated_at, tag_before.updated_at);
    }
}
