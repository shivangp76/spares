use crate::{
    Error,
    adapters::{
        SrsAdapter,
        impls::spares::{SparesAdapter, SparesRequestProcessor},
    },
    api::{
        parser::{get_parser, tests::create_parser_helper},
        review::{get_review_card, submit_study_action},
        statistics::get_statistics,
        tag::create_tag,
    },
    config::read_external_config,
    model::{Card, NEW_CARD_STATE, RatingId, Tag},
    parsers::{
        ConstructFileDataType, NoteImportAction, TemplateData, find_parser,
        generate_files::GenerateNoteFilesRequest, get_all_parsers, get_notes,
    },
    schema::{
        FilterOptions,
        note::NotesResponse,
        review::{
            GetReviewCardFilterRequest, GetReviewCardRequest, RatingSubmission, StatisticsRequest,
            StudyAction, SubmitStudyActionRequest,
        },
        tag::CreateTagRequest,
    },
    search::evaluator::Evaluator,
};
use chrono::{DateTime, Duration, Utc};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs::{create_dir_all, read_to_string},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
};

use super::note::list_notes;
use fuzz_data::generate_notes;

mod fuzz_data;
pub use fuzz_data::generate_review_logs;

const MAX_TAGS: usize = 5;
const NUM_DAYS_TO_SIMULATE_KEY: &str = "num_days_to_simulate";
const START_DATE_KEY: &str = "start_date";

#[derive(Debug)]
struct ClozeEntry {
    pub card_number: u32,
    // pub cloze_number: u32,
    pub parent: RefCell<Weak<ClozeEntry>>,
    pub children: RefCell<Vec<Rc<ClozeEntry>>>,
}

#[derive(Debug)]
struct GenerateNotesRequest {
    parser_name: String,
    scheduler_name: String,
    start_date: DateTime<Utc>,
    num_days_to_simulate: i64,
    note_count: i64,
    clozes_count: usize,
    max_nesting_level: usize,
    num_reviews: u32,
}

async fn read_generated_notes(
    pool: &SqlitePool,
    file_path: &Path,
    parser_name: &str,
) -> (NotesResponse, DateTime<Utc>) {
    // Create parser
    let _parser_response = create_parser_helper(&pool, parser_name).await;

    let parser = find_parser(parser_name, &get_all_parsers()).unwrap();
    let mut adapter = Box::new(SparesAdapter::new(SparesRequestProcessor::Database {
        pool: pool.clone(),
    }));
    let run = true;
    let quiet = true;
    let file_contents = read_to_string(file_path)
        .map_err(|e| Error::Io {
            description: format!("Failed to read {}", &file_path.display()),
            source: e,
        })
        .unwrap();
    let mut all_notes = Vec::new();
    let blocks = parser
        .start_end_regex()
        .captures_iter(file_contents.as_str())
        .map(|c| c.unwrap().get(1).unwrap().as_str())
        // .inspect(|block| dbg!(&block))
        .collect::<Vec<_>>();
    for block in blocks {
        // dbg!(&block);
        let notes = get_notes(parser.as_ref(), None, block, adapter.as_ref(), false).unwrap();
        // dbg!(&notes);
        all_notes.extend(notes);
    }
    let start_date_value = all_notes
        .first()
        .unwrap()
        .0
        .custom_data
        .get(START_DATE_KEY)
        .unwrap();
    let start_date: DateTime<Utc> = serde_json::from_value(start_date_value.clone()).unwrap();
    adapter
        .as_mut()
        .process_data(all_notes, parser.as_ref(), run, quiet, start_date)
        .await
        .unwrap();
    let notes = list_notes(
        pool,
        FilterOptions {
            page: Some(1),
            limit: Some(9999),
        },
    )
    .await
    .unwrap();
    let notes_response = NotesResponse::new(notes);
    (notes_response, start_date)
}

#[sqlx::test]
#[ignore] // ignored because takes too long
// ../../../spares/test_data/note_generator/
async fn test_simulate_reviews_1(pool: SqlitePool) {
    let output_rendered_filename = "test-9dc23620-1383-4fb1-9e41-e0cf0a4c7f0a".to_string();
    let parser_name = "markdown";
    let scheduler_name = "fsrs";
    let parser = find_parser(parser_name, &get_all_parsers()).unwrap();
    let output_text_filepath =
        get_notes_fuzz_test(&output_rendered_filename, parser.file_extension(), false);
    test_simulate_reviews_from_file(&pool, output_text_filepath, parser_name, scheduler_name).await;
}

#[sqlx::test]
async fn test_simulate_reviews_2(pool: SqlitePool) {
    let output_rendered_filename = "test-b72d8bba-cce9-4a40-b4a5-16eb5da26586".to_string();
    let parser_name = "markdown";
    let scheduler_name = "fsrs";
    let parser = find_parser(parser_name, &get_all_parsers()).unwrap();
    let output_text_filepath =
        get_notes_fuzz_test(&output_rendered_filename, parser.file_extension(), false);
    test_simulate_reviews_from_file(&pool, output_text_filepath, parser_name, scheduler_name).await;
}

fn get_notes_fuzz_test(
    output_rendered_filename: &str,
    file_extension: &str,
    temp: bool,
) -> PathBuf {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let mut output_text_filepath = PathBuf::from(&crate_dir);
    if temp {
        output_text_filepath.push("target");
    }
    output_text_filepath.push("test_data");
    output_text_filepath.push("note_generator");
    output_text_filepath.push(&output_rendered_filename);
    output_text_filepath.set_extension(file_extension);
    output_text_filepath
}

async fn test_simulate_reviews_from_file(
    pool: &SqlitePool,
    output_text_filepath: PathBuf,
    parser_name: &str,
    scheduler_name: &str,
) {
    assert!(
        output_text_filepath.exists(),
        "{} does not exist",
        output_text_filepath.display()
    );
    let (notes_response, start_date) =
        read_generated_notes(&pool, &output_text_filepath, parser_name).await;
    let num_days_to_simulate_value = notes_response
        .notes
        .first()
        .unwrap()
        .custom_data
        .get(NUM_DAYS_TO_SIMULATE_KEY)
        .unwrap();
    let num_days_to_simulate: i64 =
        serde_json::from_value(num_days_to_simulate_value.clone()).unwrap();
    simulate_reviews(
        &pool,
        notes_response,
        scheduler_name,
        start_date,
        num_days_to_simulate,
    )
    .await;
}

#[sqlx::test]
// #[ignore]
async fn test_note_generator(pool: SqlitePool) {
    let filtered_tag_query_opt = Some("tag=a or tag=b or tag=c".to_string());
    let generate_notes_request = if filtered_tag_query_opt.is_some() {
        GenerateNotesRequest {
            parser_name: "markdown".to_string(),
            scheduler_name: "fsrs".to_string(),
            start_date: Utc::now(),
            num_days_to_simulate: 1,
            note_count: 500,
            clozes_count: 2,
            max_nesting_level: 2,
            num_reviews: 50,
        }
    } else {
        GenerateNotesRequest {
            parser_name: "markdown".to_string(),
            scheduler_name: "fsrs".to_string(),
            start_date: Utc::now(),
            num_days_to_simulate: 25,
            note_count: 500,
            clozes_count: 10,
            max_nesting_level: 3,
            num_reviews: 50,
        }
    };
    let notes_response = generate_notes(&pool, &generate_notes_request).await;

    // Write input to file. This will be cleaned up later if the test succeeds.
    let grouped_notes = notes_response
        .notes
        .clone()
        .into_iter()
        .map(|note| (note.parser_id, note))
        .into_group_map();
    let mut created_files = Vec::new();
    for (parser_id, notes) in grouped_notes {
        let parser_response = get_parser(&pool, parser_id).await.unwrap();
        let parser = find_parser(parser_response.name.as_str(), &get_all_parsers()).unwrap();
        let requests = notes
            .into_iter()
            .map(|note| GenerateNoteFilesRequest {
                note_id: note.id,
                note_data: note.data,
                keywords: note.keywords,
                linked_notes: note.linked_notes,
                custom_data: note.custom_data,
                tags: note.tags,
            })
            .collect::<Vec<_>>();
        let full_requests = requests
            .iter()
            .map(|request| (ConstructFileDataType::Note, request))
            .collect::<Vec<_>>();
        let note_file_data =
            parser.construct_full_file_data(&full_requests, &NoteImportAction::Add);
        let TemplateData {
            template_contents,
            body_placeholder,
        } = parser.template_contents().unwrap();
        let file_contents = template_contents
            .as_str()
            .replace(&body_placeholder, &note_file_data);

        let output_rendered_filename = format!("test-{}", uuid::Uuid::new_v4());
        let output_text_filepath = get_notes_fuzz_test(
            output_rendered_filename.as_str(),
            parser.file_extension(),
            true,
        );
        create_dir_all(output_text_filepath.parent().unwrap()).unwrap();
        std::fs::write(&output_text_filepath, file_contents).unwrap();
        println!("Created: {}", output_text_filepath.display());
        created_files.push(output_text_filepath);
    }

    if let Some(filtered_tag_query) = filtered_tag_query_opt {
        simulate_filtered_tag_reviews(
            &pool,
            notes_response,
            &generate_notes_request.scheduler_name,
            generate_notes_request.start_date,
            generate_notes_request.num_days_to_simulate,
            &filtered_tag_query,
        )
        .await;
    } else {
        simulate_reviews(
            &pool,
            notes_response,
            &generate_notes_request.scheduler_name,
            generate_notes_request.start_date,
            generate_notes_request.num_days_to_simulate,
        )
        .await;
    }

    // Clean up files. If the test fails, the the file won't be cleaned up, so we can use it for debugging. It can also be added manually as a unit test to prevent bugs in the future.
    for file in created_files {
        trash::delete(file).unwrap();
    }
}

#[serde_with::serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
struct SimulatedReview {
    #[serde(rename = "r")]
    rating: RatingId,
    #[serde(rename = "re")]
    reviewed_at: DateTime<Utc>,
    #[serde(rename = "d")]
    #[serde_as(as = "serde_with::DurationSeconds<i64>")]
    duration: Duration,
}

async fn simulate_reviews(
    pool: &SqlitePool,
    notes_response: NotesResponse,
    scheduler_name: &str,
    start_date: DateTime<Utc>,
    num_days_to_simulate: i64,
) {
    // Create card to review history map
    let mut card_to_review_history = HashMap::new();
    for note in &notes_response.notes {
        for card_order in 1..=note.card_count {
            let review_history_value = note.custom_data.get(&card_order.to_string()).unwrap();
            let simulated_reviews: Vec<SimulatedReview> =
                serde_json::from_value(review_history_value.clone()).unwrap();
            card_to_review_history.insert((note.id, card_order as u32), simulated_reviews);
        }
    }

    let total_card_count: u32 = notes_response
        .notes
        .iter()
        .map(|note| note.card_count as u32)
        .sum();

    let mut advanced_once = false;
    let mut postponed_once = false;
    for day_offset in 0..=(num_days_to_simulate - 1) {
        dbg!(&day_offset);
        let requested_date = start_date + Duration::days(day_offset);

        // Get statistics at the start of the day
        let request = StatisticsRequest {
            scheduler_name: scheduler_name.to_string(),
            date: requested_date,
        };
        let statistics_res = get_statistics(&pool, request).await;
        assert!(statistics_res.is_ok());
        let statistics = statistics_res.unwrap();
        let config = read_external_config().unwrap();
        let mut total_due = statistics
            .due_count_by_state
            .iter()
            .map(|(_, x)| x)
            .sum::<u32>();
        if day_offset == 0 {
            assert_eq!(
                statistics.due_count_by_state.get(&NEW_CARD_STATE),
                Some(&total_card_count.min(config.new_cards_daily_limit))
            );
            assert_eq!(
                total_due,
                total_card_count.min(config.new_cards_daily_limit)
            );
            assert_eq!(statistics.advance_safe_count, 0);
            assert_eq!(statistics.postpone_safe_count, 0);
        }
        if statistics.advance_safe_count != 0 && !advanced_once {
            let request = SubmitStudyActionRequest {
                scheduler_name: scheduler_name.to_string(),
                action: StudyAction::Advance {
                    count: statistics.advance_safe_count,
                },
            };
            let advance_res = submit_study_action(&pool, request, requested_date).await;
            assert!(advance_res.is_ok());
            let request = StatisticsRequest {
                scheduler_name: scheduler_name.to_string(),
                date: requested_date,
            };
            let new_statistics_res = get_statistics(&pool, request).await;
            assert!(new_statistics_res.is_ok());
            let new_statistics = new_statistics_res.unwrap();
            let new_total_due = new_statistics
                .due_count_by_state
                .iter()
                .map(|(_, x)| x)
                .sum::<u32>();
            assert_eq!(total_due + statistics.advance_safe_count, new_total_due);
            assert!(statistics.advance_safe_count >= new_statistics.advance_safe_count);
            assert_eq!(new_statistics.advance_safe_count, 0);
            advanced_once = true;
            total_due = new_total_due;
            // We don't want to advance and postpone on the same day since these are opposing actions. Also, advancing cards will causes more cards to be able to be postponed.
        } else if statistics.postpone_safe_count != 0 && !postponed_once {
            let request = SubmitStudyActionRequest {
                scheduler_name: scheduler_name.to_string(),
                action: StudyAction::Postpone {
                    count: statistics.postpone_safe_count,
                },
            };
            let postpone_res = submit_study_action(&pool, request, requested_date).await;
            assert!(postpone_res.is_ok());
            let request = StatisticsRequest {
                scheduler_name: scheduler_name.to_string(),
                date: requested_date,
            };
            let new_statistics_res = get_statistics(&pool, request).await;
            assert!(new_statistics_res.is_ok());
            let new_statistics = new_statistics_res.unwrap();
            let new_total_due = new_statistics
                .due_count_by_state
                .iter()
                .map(|(_, x)| x)
                .sum::<u32>();
            assert_eq!(total_due - statistics.postpone_safe_count, new_total_due);
            assert!(statistics.postpone_safe_count >= new_statistics.postpone_safe_count);
            assert_eq!(new_statistics.postpone_safe_count, 0);
            postponed_once = true;
            total_due = new_total_due;
        }

        // Submit reviews
        let mut flips_count = 0;
        loop {
            // Get review
            let review_res = get_review_card(
                &pool,
                GetReviewCardRequest { filter: None },
                requested_date,
                &get_all_parsers(),
            )
            .await;
            assert!(review_res.is_ok());
            let review_card_opt = review_res.unwrap();
            if let Some(review_card) = review_card_opt {
                // Submit random reviews
                let reviews = card_to_review_history
                    .get_mut(&(review_card.note_id, review_card.card_order))
                    .unwrap();
                let review_opt = reviews.pop();
                assert!(review_opt.is_some(), "hint: increase `num_reviews`");
                let SimulatedReview {
                    rating,
                    reviewed_at: _,
                    duration,
                } = review_opt.unwrap();
                let request = SubmitStudyActionRequest {
                    scheduler_name: scheduler_name.to_string(),
                    action: StudyAction::Rate(RatingSubmission {
                        card_id: review_card.card_id,
                        rating,
                        duration,
                        tag_id: None,
                    }),
                };
                let submit_review_res = submit_study_action(&pool, request, requested_date).await;
                assert!(submit_review_res.is_ok());
                flips_count += 1;
            } else {
                break;
            }
        }
        assert!(flips_count >= total_due);

        // Get statistics at the end of the day
        // Only done occasionally to speed up the test
        if day_offset % 10 == 0 {
            let request = StatisticsRequest {
                scheduler_name: scheduler_name.to_string(),
                date: requested_date,
            };
            let statistics_res = get_statistics(&pool, request).await;
            assert!(statistics_res.is_ok());
            let statistics = statistics_res.unwrap();
            assert!(statistics.cards_studied_count > 0);
            assert!(statistics.study_time > Duration::zero());
            assert!(statistics.due_count_by_state.is_empty());
        }
    }
    assert!(advanced_once);
    assert!(postponed_once);

    // TODO: Use the code above to examine and unit test for:
    // - smart schedule: Examine the distribution of reviews on different days to see if it lines up with the workload_percentage
}

async fn simulate_filtered_tag_reviews(
    pool: &SqlitePool,
    notes_response: NotesResponse,
    scheduler_name: &str,
    start_date: DateTime<Utc>,
    num_days_to_simulate: i64,
    filtered_tag_query: &str,
) {
    // Create card to review history map
    let mut card_to_review_history = HashMap::new();
    for note in &notes_response.notes {
        for card_order in 1..=note.card_count {
            let review_history_value = note.custom_data.get(&card_order.to_string()).unwrap();
            let simulated_reviews: Vec<SimulatedReview> =
                serde_json::from_value(review_history_value.clone()).unwrap();
            card_to_review_history.insert((note.id, card_order as u32), simulated_reviews);
        }
    }

    // Create filtered tag
    let request = CreateTagRequest {
        name: "test-filtered-tag".to_string(),
        description: String::new(),
        parent_id: None,
        query: Some(filtered_tag_query.to_string()),
        auto_delete: true,
    };
    let tag_res = create_tag(&pool, request).await;
    assert!(tag_res.is_ok());
    let filtered_tag_id = tag_res.unwrap().id;

    // Get total number of cards that are a part of the filtered tag. Use this to test searching by filtered tag.
    let query = "tag=\"test-filtered-tag\"";
    let evaluator = Evaluator::new(query);
    let cards_matching_filtered_tag = evaluator.get_card_ids(&pool).await.unwrap();

    let mut reviewed_card_ids = HashSet::new();
    for day_offset in 0..=(num_days_to_simulate - 1) {
        dbg!(&day_offset);
        let requested_date = start_date + Duration::days(day_offset);

        // Submit reviews
        let mut review_count = 0;
        loop {
            // Get review
            let review_res = get_review_card(
                &pool,
                GetReviewCardRequest {
                    filter: Some(GetReviewCardFilterRequest::FilteredTag {
                        tag_id: filtered_tag_id,
                    }),
                },
                requested_date,
                &get_all_parsers(),
            )
            .await;
            assert!(review_res.is_ok());
            let review_card_opt = review_res.unwrap();
            if let Some(review_card) = review_card_opt {
                // Submit random reviews
                let reviews = card_to_review_history
                    .get_mut(&(review_card.note_id, review_card.card_order))
                    .unwrap();
                let review_opt = reviews.pop();
                assert!(review_opt.is_some(), "hint: increase `num_reviews`");
                let SimulatedReview {
                    rating,
                    reviewed_at: _,
                    duration,
                } = review_opt.unwrap();
                let request = SubmitStudyActionRequest {
                    scheduler_name: scheduler_name.to_string(),
                    action: StudyAction::Rate(RatingSubmission {
                        card_id: review_card.card_id,
                        rating,
                        duration,
                        tag_id: Some(filtered_tag_id),
                    }),
                };
                let submit_review_res = submit_study_action(&pool, request, requested_date).await;
                assert!(submit_review_res.is_ok());
                review_count += 1;
                reviewed_card_ids.insert(review_card.card_id);
                if review_count <= 3 {
                    // Validate that the card has custom data
                    let card: Card = sqlx::query_as(r"SELECT * FROM card WHERE id = ?")
                        .bind(review_card.card_id)
                        .fetch_one(pool)
                        .await
                        .unwrap();
                    // `rating` is `Rating::Again` or `Rating::Hard`
                    if rating == 0 || rating == 1 {
                        let custom_data = card.custom_data.as_object().unwrap();
                        let tag_id_str = filtered_tag_id.to_string();
                        let filtered_tag_custom_data = custom_data.get(&tag_id_str);
                        assert!(filtered_tag_custom_data.is_some());
                    }
                }
                if review_count <= 30 {
                    // `rating` is `Rating::Easy`
                    if rating == 4 {
                        // Validate that there is at least 1 less card that is a part of the filtered tag
                        let evaluator = Evaluator::new(query);
                        let cards_matching_filtered_tag_after =
                            evaluator.get_card_ids(&pool).await.unwrap();
                        assert!(
                            cards_matching_filtered_tag_after.len()
                                < cards_matching_filtered_tag.len()
                        );
                    }
                }
            } else {
                break;
            }
        }
    }

    // Validate filtered tag is auto deleted
    let tag_opt: Option<Tag> = sqlx::query_as(r"SELECT * FROM tag WHERE id = ?")
        .bind(filtered_tag_id)
        .fetch_optional(pool)
        .await
        .unwrap();
    assert!(tag_opt.is_none());

    // Verify all cards that match the filtered tag were reviewed
    assert!(!reviewed_card_ids.is_empty());
    assert_eq!(reviewed_card_ids.len(), cards_matching_filtered_tag.len());
}
