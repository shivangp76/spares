use super::{ClozeEntry, MAX_TAGS};
use crate::{
    api::{
        note::create_notes,
        parser::tests::create_parser_helper,
        tests::{GenerateNotesRequest, NUM_DAYS_TO_SIMULATE_KEY, START_DATE_KEY, SimulatedReview},
    },
    model::{Card, ReviewLog},
    parsers::{
        BackReveal, ClozeGrouping, ClozeGroupingSettings, ClozeSettings, FrontConceal,
        NoteSettingsKeys, Parseable, construct_cloze_string, find_parser, get_all_parsers,
    },
    schedulers::{SrsScheduler, get_scheduler_from_string},
    schema::note::{CreateNoteRequest, CreateNotesRequest, NotesResponse},
};
use rand::{Rng, rngs::ThreadRng, seq::SliceRandom};
use serde_json::Map;
use sqlx::SqlitePool;
use std::{
    cell::RefCell,
    collections::HashSet,
    rc::{Rc, Weak},
};

fn generate_next_cloze_options(mut input: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    input.sort();
    let last = input.last().copied().unwrap_or((0, 0));
    let mut result = vec![(last.0 + 1, 1)];
    let mut seen_majors = HashSet::new();
    for (major, minor) in input.into_iter().rev() {
        if !seen_majors.contains(&major) {
            seen_majors.insert(major);
            result.push((major, minor + 1));
        }
    }
    result.reverse();
    result
}

#[test]
fn test_single_entry() {
    let input = vec![(1, 1)];
    let expected = vec![(1, 2), (2, 1)];
    assert_eq!(generate_next_cloze_options(input), expected);
}

#[test]
fn test_multiple_same_major() {
    let input = vec![(1, 1), (1, 2)];
    let expected = vec![(1, 3), (2, 1)];
    assert_eq!(generate_next_cloze_options(input), expected);
}

#[test]
fn test_multiple_different_major() {
    let input = vec![(1, 1), (2, 1)];
    let expected = vec![(1, 2), (2, 2), (3, 1)];
    assert_eq!(generate_next_cloze_options(input), expected);
}

#[test]
fn test_empty_input() {
    let input = vec![];
    let expected = vec![(1, 1)];
    assert_eq!(generate_next_cloze_options(input), expected);
}

#[test]
fn test_large_minor() {
    let input = vec![(1, 9)];
    let expected = vec![(1, 10), (2, 1)];
    assert_eq!(generate_next_cloze_options(input), expected);
}

#[test]
fn test_unsorted() {
    let input = vec![
        (1, 1),
        (2, 1),
        (1, 2),
        (2, 2),
        (3, 1),
        (4, 1),
        (5, 1),
        (3, 2),
        (4, 1),
    ];
    let expected = vec![(1, 3), (2, 3), (3, 3), (4, 2), (5, 2), (6, 1)];
    assert_eq!(generate_next_cloze_options(input), expected);
}

// #[derive(Debug, PartialEq)]
// struct ClozeEntry {
//     pub card_number: u32,
//     pub cloze_number: u32,
//     pub nesting_level: u32,
// }
//
// fn generate_note(clozes_count: u32, max_nesting_level: u32) -> Vec<ClozeEntry> {
//     let mut clozes: Vec<ClozeEntry> = Vec::new();
//     for _ in 1..=clozes_count {
//         // Nesting level
//         let previous_nesting_level = clozes.last().map(|x| x.nesting_level).unwrap_or(0);
//         let nesting_options = (0..(previous_nesting_level + 1).min(max_nesting_level))
//             .into_iter()
//             .collect::<Vec<_>>();
//         let nesting_level = nesting_options.choose(&mut rand::thread_rng()).unwrap();
//
//         // Card and cloze number
//         let current_clozes = clozes
//             .iter()
//             .map(
//                 |ClozeEntry {
//                      card_number,
//                      cloze_number,
//                      ..
//                  }| (*card_number, *cloze_number),
//             )
//             .collect::<Vec<_>>();
//         let mut card_cloze_options = generate_next_cloze_options(&current_clozes);
//
//         // Clozes part of the same card cannot be nested, so remove those
//         let count = if *nesting_level == 0 {
//             0
//         } else {
//             nesting_level - 1
//         };
//         let ancestors = (0..=count)
//             .into_iter()
//             .rev()
//             .map(|nl_search| {
//                 clozes
//                     .iter()
//                     .rev()
//                     .find(|cloze| cloze.nesting_level == nl_search)
//             })
//             .flatten()
//             .map(|c| (c.card_number, c.cloze_number))
//             .collect::<Vec<_>>();
//         card_cloze_options.retain(|x| !ancestors.contains(&x));
//         let card_cloze = card_cloze_options.choose(&mut rand::thread_rng()).unwrap();
//
//         clozes.push(ClozeEntry {
//             card_number: card_cloze.0,
//             cloze_number: card_cloze.1,
//             nesting_level: *nesting_level,
//         });
//     }
//     clozes
// }

fn get_last_children(current: &Rc<ClozeEntry>) -> Vec<Rc<ClozeEntry>> {
    let mut result = Vec::new();
    // let mut current_node = Some(current);
    // while let Some(cur_node) = current_node {
    //     result.push(cur_node);
    //     let a = cur_node.children.borrow();
    //     current_node = cur_node.children.borrow().last();
    // }
    let children = current.children.borrow();
    if children.is_empty() {
        return vec![Rc::clone(current)];
    }
    result.push(Rc::clone(&current));
    let last_child = children.last().unwrap();
    result.extend(get_last_children(&Rc::clone(last_child)));
    result
}

fn generate_note_structure(
    clozes_count: usize,
    max_nesting_level: usize,
    mut rng: &mut ThreadRng,
) -> Rc<ClozeEntry> {
    let root = Rc::new(ClozeEntry {
        card_number: 0,
        // cloze_number: 0,
        parent: RefCell::new(Weak::new()),
        children: RefCell::new(Vec::new()),
    });
    let mut current_used_clozes = Vec::with_capacity(clozes_count);
    for _ in 1..=clozes_count {
        // Nesting level
        let ancestors = get_last_children(&root);
        let capped_ancestors = ancestors.iter().take(max_nesting_level).collect::<Vec<_>>();
        let parent = *capped_ancestors.choose(&mut rng).unwrap();

        // Card and cloze number
        let mut card_cloze_options = generate_next_cloze_options(current_used_clozes.clone());
        // Clozes part of the same card cannot be nested, so remove those
        card_cloze_options.retain(|(card_number, _)| {
            ancestors
                .iter()
                .find(|x| x.card_number == *card_number)
                .is_none()
        });
        let card_cloze = card_cloze_options.choose(&mut rng).unwrap();
        current_used_clozes.push(*card_cloze);

        let new_cloze = Rc::new(ClozeEntry {
            card_number: card_cloze.0,
            // cloze_number: card_cloze.1,
            parent: RefCell::new(Weak::new()),
            children: RefCell::new(Vec::new()),
        });
        *new_cloze.parent.borrow_mut() = Rc::downgrade(&parent);
        parent.children.borrow_mut().push(Rc::clone(&new_cloze));
    }
    root
}

fn generate_tags() -> Vec<String> {
    let all_tags: Vec<String> = ('a'..='z').map(|c| c.to_string()).collect();
    let max_options = all_tags.len().min(MAX_TAGS);
    let mut rng = rand::thread_rng();
    let chosen_num_tags = rng.gen_range(0..=max_options);
    let mut sampled = all_tags.to_vec();
    sampled.shuffle(&mut rng);
    sampled.truncate(chosen_num_tags);
    sampled
}

fn generate_note(node: Rc<ClozeEntry>, parser: &dyn Parseable, mut rng: &mut ThreadRng) -> String {
    let mut result = String::new();
    let children = node.children.borrow();
    let include_forward_card = *[true, false].choose(&mut rng).unwrap();
    let include_backward_card = if !include_forward_card {
        true
    } else {
        *[true, false].choose(&mut rng).unwrap()
    };
    let is_suspended = *[true, false].choose(&mut rng).unwrap();
    let cloze_settings = ClozeSettings::default();
    let grouping_settings = ClozeGroupingSettings {
        grouping: ClozeGrouping::Custom(node.card_number.to_string()),
        orders: None,
        include_forward_card,
        include_backward_card,
        is_suspended: Some(is_suspended),
        // NOTE: For simplicity, no clozes are hidden. This is because otherwise you have to make sure that all clozes in the same grouping are not all hidden. This requires knowing the data for all other clozes in the grouping which is more complicated.
        hidden_no_answer: false,
        front_conceal: FrontConceal::OnlyGrouping,
        back_reveal: BackReveal::FullNote,
        hidden: false,
    };
    // NOTE: For simplicity, each cloze is only a part of 1 grouping.
    let NoteSettingsKeys {
        settings_delim,
        settings_key_value_delim,
        ..
    } = parser.note_settings_keys();
    let cloze_settings_string = construct_cloze_string(
        &cloze_settings,
        &[grouping_settings],
        &parser.cloze_settings_keys(),
        settings_delim,
        settings_key_value_delim,
        None,
    );

    let mut cloze_body = String::new();
    for child in children.iter() {
        let child_string = generate_note(Rc::clone(child), parser, rng);
        cloze_body.push_str(child_string.as_str());
    }
    let body_string = format!("\nExpected Card {}\n", node.card_number);
    cloze_body.push_str(body_string.as_str());

    let (cloze_prefix, cloze_suffix) = parser.construct_cloze(&cloze_settings_string, &cloze_body);
    if node.card_number == 0 {
        result.push_str(&cloze_body);
    } else {
        result.push_str(&cloze_prefix);
        result.push_str(&cloze_body);
        result.push_str(&cloze_suffix);
    }

    result
}

pub async fn generate_notes(
    pool: &SqlitePool,
    generate_notes_request: &GenerateNotesRequest,
) -> NotesResponse {
    let GenerateNotesRequest {
        parser_name,
        scheduler_name,
        start_date,
        num_days_to_simulate,
        note_count,
        clozes_count,
        max_nesting_level,
        num_reviews,
    } = generate_notes_request;
    let scheduler = get_scheduler_from_string(&scheduler_name).unwrap();
    let parser_response = create_parser_helper(&pool, &parser_name).await;
    let parser = find_parser(&parser_name, &get_all_parsers()).unwrap();
    let mut rng = rand::thread_rng();
    let create_note_requests = (1..=*note_count)
        .into_iter()
        .map(|_| {
            let note_structure =
                generate_note_structure(*clozes_count, *max_nesting_level, &mut rng);
            generate_note(note_structure, parser.as_ref(), &mut rng)
        })
        .map(|note_data| CreateNoteRequest {
            data: note_data,
            keywords: Vec::new(),
            tags: generate_tags(),
            is_suspended: false,
            custom_data: Map::new(),
        })
        .collect::<Vec<_>>();
    let request = CreateNotesRequest {
        parser_id: parser_response.id,
        requests: create_note_requests,
    };
    let note_responses_res = create_notes(&pool, request, *start_date, &get_all_parsers()).await;
    assert!(note_responses_res.is_ok());
    let mut note_responses = note_responses_res.unwrap();

    for note in &mut note_responses.notes {
        let card_count = note.card_count as u32;

        // Generate review history for all cards
        let first_review_date = note.created_at;
        let review_histories = scheduler.generate_review_history(
            card_count,
            *num_reviews,
            first_review_date,
            &mut rng,
        );

        let mut custom_data = Map::new();
        custom_data.insert(
            NUM_DAYS_TO_SIMULATE_KEY.to_string(),
            serde_json::to_value(num_days_to_simulate).unwrap(),
        );
        custom_data.insert(
            START_DATE_KEY.to_string(),
            serde_json::to_value(start_date).unwrap(),
        );
        for (i, mut review_history) in review_histories.into_iter().enumerate() {
            let card_order = (i + 1) as u32;
            review_history.reverse();
            let simulated_reviews = review_history
                .into_iter()
                .map(|(review, reviewed_at)| SimulatedReview {
                    rating: review.rating,
                    reviewed_at,
                    duration: review.duration,
                })
                .collect::<Vec<_>>();
            custom_data.insert(
                card_order.to_string(),
                serde_json::to_value(simulated_reviews).unwrap(),
            );
        }
        note.custom_data = custom_data;
    }
    note_responses
}

pub fn generate_review_logs(
    scheduler: &dyn SrsScheduler,
    initial_card: Card,
    mut rng: &mut ThreadRng,
) -> (Card, Vec<ReviewLog>) {
    let num_siblings = 1;
    let num_reviews = rng.gen_range(3..=5);
    let first_review_date = initial_card.created_at;
    let review_histories_all =
        scheduler.generate_review_history(num_siblings, num_reviews, first_review_date, &mut rng);
    let review_histories = &review_histories_all[0];
    let (card, review_logs) =
        review_histories
            .iter()
            .fold((initial_card, Vec::new()), |acc, (review, reviewed_at)| {
                let (card, mut review_logs): (_, Vec<ReviewLog>) = acc;
                let previous_review_log = review_logs.last();
                let (new_card, new_review_log) = scheduler
                    .schedule(
                        &card,
                        previous_review_log.cloned(),
                        review.rating,
                        *reviewed_at,
                        review.duration,
                    )
                    .unwrap();
                review_logs.push(new_review_log);
                (new_card, review_logs)
            });
    (card, review_logs)
}
