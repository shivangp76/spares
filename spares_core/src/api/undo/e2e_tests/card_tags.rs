//! Tags on cards themselves, and their undo

use chrono::Utc;
use sqlx::SqlitePool;

use super::create_card_helper;
use crate::api::card::update_cards;
use crate::api::card_tags::card_tag_names;
use crate::api::card_tags::update_card_tags;
use crate::api::tag::create_tag;
use crate::api::undo::redo_event;
use crate::api::undo::undo_event;
use crate::model::CardId;
use crate::model::SpecialState;
use crate::schema::card::CardsSelector;
use crate::schema::card::SpecialStateUpdate;
use crate::schema::card::UpdateCardsRequest;
use crate::schema::note::UpdateTags;
use crate::schema::tag::CreateTagRequest;
use crate::schema::undo::RedoEventRequest;
use crate::schema::undo::UndoEventRequest;
use crate::search::evaluator::Evaluator;

fn add(tags: &[&str]) -> UpdateTags {
    UpdateTags::ModifyTags {
        tags_to_remove: None,
        tags_to_add: Some(tags.iter().map(ToString::to_string).collect()),
    }
}

fn remove(tags: &[&str]) -> UpdateTags {
    UpdateTags::ModifyTags {
        tags_to_remove: Some(tags.iter().map(ToString::to_string).collect()),
        tags_to_add: None,
    }
}

async fn tags_of(pool: &SqlitePool, card_id: CardId) -> Vec<String> {
    card_tag_names(pool, &[card_id])
        .await
        .unwrap()
        .remove(&card_id)
        .unwrap_or_default()
}

async fn tag_exists(pool: &SqlitePool, name: &str) -> bool {
    sqlx::query_scalar(r"SELECT COUNT(*) > 0 FROM tag WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn undo_group(pool: &SqlitePool) {
    undo_event(
        pool,
        UndoEventRequest {
            event_id: None,
            undo_group: true,
        },
    )
    .await
    .unwrap()
    .unwrap();
}

#[sqlx::test]
async fn tagging_a_card_creates_the_tag_and_undo_and_redo_reverse_it(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;
    let event_id = update_card_tags(&pool, &[card_id], &add(&["hard"]), Utc::now(), true)
        .await
        .unwrap();
    assert!(event_id.is_some());
    assert_eq!(tags_of(&pool, card_id).await, ["hard"]);
    // `tag=` finds cards by their own tags, as well as their notes'
    let found = Evaluator::new("tag=hard")
        .get_card_ids(&pool)
        .await
        .unwrap();
    assert_eq!(found, [card_id]);

    undo_group(&pool).await;
    assert_eq!(tags_of(&pool, card_id).await, Vec::<String>::new());
    assert!(
        !tag_exists(&pool, "hard").await,
        "the tag the tagging created goes with it"
    );

    redo_event(
        &pool,
        RedoEventRequest {
            event_id: None,
            redo_group: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(tags_of(&pool, card_id).await, ["hard"]);
}

#[sqlx::test]
async fn untagging_the_last_card_deletes_the_tag_and_undo_restores_it(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;
    update_card_tags(&pool, &[card_id], &add(&["hard"]), Utc::now(), false)
        .await
        .unwrap();
    let tag_id: i64 = sqlx::query_scalar(r"SELECT id FROM tag WHERE name = 'hard'")
        .fetch_one(&pool)
        .await
        .unwrap();

    update_card_tags(&pool, &[card_id], &remove(&["hard"]), Utc::now(), true)
        .await
        .unwrap();
    assert!(!tag_exists(&pool, "hard").await);

    undo_group(&pool).await;
    assert_eq!(tags_of(&pool, card_id).await, ["hard"]);
    let restored_id: i64 = sqlx::query_scalar(r"SELECT id FROM tag WHERE name = 'hard'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(restored_id, tag_id, "the tag is restored as it was");
}

#[sqlx::test]
async fn filtered_tags_cannot_be_given_to_cards_by_hand(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;
    create_tag(
        &pool,
        CreateTagRequest {
            name: "exam".to_string(),
            description: String::new(),
            query: Some("c.due<=today".to_string()),
            auto_delete: false,
            reschedule: true,
        },
        false,
    )
    .await
    .unwrap();
    let filtered_before = card_tag_names(&pool, &[card_id]).await.unwrap();

    let result = update_cards(
        &pool,
        UpdateCardsRequest {
            selector: CardsSelector::Ids(vec![card_id]),
            desired_retention: None,
            special_state: Some(Some(SpecialStateUpdate::Suspended)),
            due: None,
            tags: add(&["exam"]),
        },
        Utc::now(),
        true,
    )
    .await;
    assert!(result.is_err());
    let special_state: Option<SpecialState> =
        sqlx::query_scalar(r"SELECT special_state FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        special_state, None,
        "nothing changes when tagging is refused"
    );
    assert_eq!(
        card_tag_names(&pool, &[card_id]).await.unwrap(),
        filtered_before
    );
}

#[sqlx::test]
async fn one_undo_reverses_a_card_edit_that_also_tags(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;
    let response = update_cards(
        &pool,
        UpdateCardsRequest {
            selector: CardsSelector::Ids(vec![card_id]),
            desired_retention: None,
            special_state: Some(Some(SpecialStateUpdate::Suspended)),
            due: None,
            tags: add(&["hard", "math:algebra", "math"]),
        },
        Utc::now(),
        true,
    )
    .await
    .unwrap();
    assert!(response.event_id.is_some());
    // An ancestor of another of its tags is left out, as on notes
    assert_eq!(tags_of(&pool, card_id).await, ["hard", "math:algebra"]);

    undo_group(&pool).await;
    assert_eq!(tags_of(&pool, card_id).await, Vec::<String>::new());
    let special_state: Option<SpecialState> =
        sqlx::query_scalar(r"SELECT special_state FROM card WHERE id = ?")
            .bind(card_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(special_state, None);
}

#[sqlx::test]
async fn a_request_that_only_tags_logs_no_card_update(pool: SqlitePool) {
    let card_id = create_card_helper(&pool).await;
    update_cards(
        &pool,
        UpdateCardsRequest {
            selector: CardsSelector::Ids(vec![card_id]),
            desired_retention: None,
            special_state: None,
            due: None,
            tags: add(&["hard"]),
        },
        Utc::now(),
        true,
    )
    .await
    .unwrap();
    let kinds: Vec<crate::model::EventType> =
        sqlx::query_scalar(r"SELECT kind FROM event WHERE kind != ? ORDER BY id")
            .bind(crate::model::EventType::CreateParser)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        kinds,
        [
            crate::model::EventType::CreateTag,
            crate::model::EventType::UpdateCardTags
        ]
    );
}
