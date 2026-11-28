//! Optimizing the scheduler's parameters: saving them to the collection's settings and
//! rescheduling, as one `OptimizeScheduler` event that undo reverts entirely, and
//! `auto_optimize`'s proposals.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Duration;
use chrono::Utc;
use sqlx::SqlitePool;

use crate::api::scheduler::apply_optimize_proposal;
use crate::api::scheduler::auto_optimize;
use crate::api::scheduler::auto_optimize_due;
use crate::api::scheduler::discard_optimize_proposal;
use crate::api::scheduler::optimize_scheduler;
use crate::api::scheduler::optimize_status;
use crate::api::scheduler::tests::seed_review_history;
use crate::api::undo::redo_event;
use crate::api::undo::undo_event;
use crate::collection::Collection;
use crate::collection::CollectionPaths;
use crate::config::read_external_config;
use crate::model::CardId;
use crate::model::EventType;
use crate::schema::review::OptimizeRequest;
use crate::schema::undo::RedoEventRequest;
use crate::schema::undo::UndoEventRequest;

/// Runs `test` in a collection of its own, whose `config.toml` starts as `config`.
async fn in_collection<F: Future<Output = ()>>(
    pool: &SqlitePool,
    config: &str,
    test: impl FnOnce() -> F,
) {
    let root = tempfile::tempdir().unwrap();
    let paths = CollectionPaths::in_dir(root.path());
    std::fs::create_dir_all(&paths.config).unwrap();
    std::fs::write(paths.config_file(), config).unwrap();
    let collection = Arc::new(Collection::new("optimize", pool.clone(), paths));
    collection.set_replica(Some(crate::config::Replica::ROOT));
    collection.scope(test()).await;
    drop(root);
}

/// Each card's due date and stability.
async fn schedules(pool: &SqlitePool) -> HashMap<CardId, (i64, f64)> {
    sqlx::query_as::<_, (CardId, i64, f64)>("SELECT id, due, stability FROM card")
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|(id, due, stability)| (id, (due, stability)))
        .collect()
}

const HAND_SET: &str = "# My settings\nload_balance = true\nfsrs_parameters = [0.212, 1.2931, 2.3065, 8.2956, 6.4133, 0.8334, 3.0194, 0.001, 1.8722, 0.1666, 0.796, 1.4835, 0.0614, 0.2629, 1.6483, 0.6014, 1.8729, 0.5425, 0.0912, 0.0658, 0.1542]\n";

#[sqlx::test]
async fn undoing_an_optimize_restores_the_parameters_and_the_cards(pool: SqlitePool) {
    in_collection(&pool, HAND_SET, || async {
        seed_review_history(&pool, 40, Utc::now() - Duration::days(60)).await;
        let hand_set = read_external_config().unwrap().fsrs_parameters;
        let before = schedules(&pool).await;

        let response =
            optimize_scheduler(&pool, "fsrs", OptimizeRequest { apply: true }, Utc::now())
                .await
                .unwrap();
        assert!(response.applied, "{response:?}");
        let event_id = response.event_id.expect("an OptimizeScheduler event");
        let kind: EventType = sqlx::query_scalar("SELECT kind FROM event WHERE id = ?")
            .bind(event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(kind, EventType::OptimizeScheduler);
        let config = read_external_config().unwrap();
        assert_eq!(config.fsrs_parameters, response.optimized_parameters);
        assert!(config.fsrs_parameters_fitted_at.is_some());
        assert_eq!(
            config.fsrs_parameters_review_count,
            Some(response.review_count)
        );
        let optimized = schedules(&pool).await;
        assert_ne!(optimized, before, "the reschedule moved cards");

        // Not automatic, so a plain undo reaches it
        undo_event(
            &pool,
            UndoEventRequest {
                event_id: None,
                undo_group: false,
            },
        )
        .await
        .unwrap()
        .expect("undone");
        let config = read_external_config().unwrap();
        assert_eq!(
            config.fsrs_parameters, hand_set,
            "the hand-set parameters are back"
        );
        assert_eq!(
            config.fsrs_parameters_fitted_at, None,
            "and count as never optimized"
        );
        assert_eq!(config.fsrs_parameters_review_count, None);
        assert_eq!(schedules(&pool).await, before);
        let settings = crate::settings::cached().unwrap();
        assert!(
            settings.contains("# My settings"),
            "the rest of the settings is kept: {settings}"
        );

        redo_event(
            &pool,
            RedoEventRequest {
                event_id: None,
                redo_group: false,
            },
        )
        .await
        .unwrap()
        .expect("redone");
        let config = read_external_config().unwrap();
        assert_eq!(config.fsrs_parameters, response.optimized_parameters);
        assert_eq!(
            config.fsrs_parameters_review_count,
            Some(response.review_count)
        );
        assert_eq!(schedules(&pool).await, optimized);
    })
    .await;
}

#[sqlx::test]
async fn an_optimize_is_not_undone_over_parameters_changed_since(pool: SqlitePool) {
    in_collection(&pool, "", || async {
        seed_review_history(&pool, 40, Utc::now() - Duration::days(60)).await;
        let response =
            optimize_scheduler(&pool, "fsrs", OptimizeRequest { apply: true }, Utc::now())
                .await
                .unwrap();
        // Set by hand afterwards
        let path = crate::collection::paths().config_file();
        std::fs::write(&path, HAND_SET).unwrap();
        let error = undo_event(
            &pool,
            UndoEventRequest {
                event_id: response.event_id,
                undo_group: false,
            },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("changed since"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            HAND_SET,
            "left alone"
        );
    })
    .await;
}

#[sqlx::test]
async fn status_counts_the_reviews_since_the_parameters_were_fitted(pool: SqlitePool) {
    in_collection(&pool, HAND_SET, || async {
        seed_review_history(&pool, 120, Utc::now() - Duration::days(60)).await;
        let status = optimize_status(&pool, "fsrs", Utc::now()).await.unwrap();
        assert!(status.optimizable);
        assert_eq!(status.review_count, 600);
        assert_eq!(
            status.fitted_at, None,
            "hand-set parameters were never optimized"
        );
        assert_eq!(status.reviews_since, 600);
        assert!(status.suggested, "{status:?}");

        optimize_scheduler(&pool, "fsrs", OptimizeRequest { apply: true }, Utc::now())
            .await
            .unwrap();
        let status = optimize_status(&pool, "fsrs", Utc::now()).await.unwrap();
        assert_eq!(status.reviews_since, 0);
        assert_eq!(status.days_since, Some(0));
        assert!(!status.suggested);
        // A month later, with reviews since
        seed_review_history(&pool, 1, Utc::now() - Duration::days(1)).await;
        let status = optimize_status(&pool, "fsrs", Utc::now() + Duration::days(31))
            .await
            .unwrap();
        assert_eq!(status.reviews_since, 5);
        assert!(status.suggested, "{status:?}");
    })
    .await;
}

#[sqlx::test]
async fn auto_optimize_proposes_parameters_and_changes_nothing_until_applied(pool: SqlitePool) {
    in_collection(
        &pool,
        &format!("auto_optimize = true\n{HAND_SET}"),
        || async {
            seed_review_history(&pool, 120, Utc::now() - Duration::days(60)).await;
            let hand_set = read_external_config().unwrap().fsrs_parameters;
            let before = schedules(&pool).await;
            assert!(auto_optimize_due(&pool, "fsrs", Utc::now()).await.unwrap());

            let proposal = auto_optimize(&pool, "fsrs", Utc::now())
                .await
                .unwrap()
                .expect("parameters fitted to a history this forgetful predict it clearly better");
            assert_eq!(
                read_external_config().unwrap().fsrs_parameters,
                hand_set,
                "nothing saved"
            );
            assert_eq!(schedules(&pool).await, before, "nothing moved");
            let status = optimize_status(&pool, "fsrs", Utc::now()).await.unwrap();
            assert!(status.auto_optimize);
            assert_eq!(
                status.proposal.as_ref().map(|p| &p.parameters),
                Some(&proposal.parameters)
            );
            assert!(!status.suggested, "the proposal is offered instead");
            assert!(!auto_optimize_due(&pool, "fsrs", Utc::now()).await.unwrap());

            // Discarded, it isn't fitted again until as many reviews or days have passed
            discard_optimize_proposal(&pool).await.unwrap();
            let status = optimize_status(&pool, "fsrs", Utc::now()).await.unwrap();
            assert!(status.proposal.is_none());
            assert!(!status.suggested, "{status:?}");
            assert!(!auto_optimize_due(&pool, "fsrs", Utc::now()).await.unwrap());

            // Applied, it is an optimize like any other, and undone with its parameters
            auto_optimize(&pool, "fsrs", Utc::now()).await.unwrap();
            let applied = apply_optimize_proposal(&pool, "fsrs", Utc::now())
                .await
                .unwrap();
            let config = read_external_config().unwrap();
            assert_eq!(config.fsrs_parameters, proposal.parameters);
            assert_eq!(config.fsrs_parameters_review_count, Some(600));
            assert!(
                optimize_status(&pool, "fsrs", Utc::now())
                    .await
                    .unwrap()
                    .proposal
                    .is_none()
            );
            assert!(
                apply_optimize_proposal(&pool, "fsrs", Utc::now())
                    .await
                    .is_err(),
                "applied once"
            );
            undo_event(
                &pool,
                UndoEventRequest {
                    event_id: applied.event_id,
                    undo_group: false,
                },
            )
            .await
            .unwrap();
            assert_eq!(read_external_config().unwrap().fsrs_parameters, hand_set);
            assert_eq!(schedules(&pool).await, before);
        },
    )
    .await;
}

#[sqlx::test]
async fn a_host_with_an_upstream_never_optimizes_by_itself(pool: SqlitePool) {
    let config =
        format!("auto_optimize = true\n{HAND_SET}\n[sync]\nurl = \"https://example.com\"\n");
    in_collection(&pool, &config, || async {
        seed_review_history(&pool, 120, Utc::now() - Duration::days(60)).await;
        assert!(!auto_optimize_due(&pool, "fsrs", Utc::now()).await.unwrap());
        let status = optimize_status(&pool, "fsrs", Utc::now()).await.unwrap();
        assert!(!status.auto_optimize);
        assert!(status.suggested, "it only suggests optimizing");
    })
    .await;
}

#[sqlx::test]
async fn a_proposal_scored_against_parameters_since_replaced_is_dropped(pool: SqlitePool) {
    in_collection(
        &pool,
        &format!("auto_optimize = true\n{HAND_SET}"),
        || async {
            seed_review_history(&pool, 120, Utc::now() - Duration::days(60)).await;
            auto_optimize(&pool, "fsrs", Utc::now())
                .await
                .unwrap()
                .unwrap();
            std::fs::write(
                crate::collection::paths().config_file(),
                "auto_optimize = true\n",
            )
            .unwrap();
            let status = optimize_status(&pool, "fsrs", Utc::now()).await.unwrap();
            assert!(status.proposal.is_none());
            assert!(
                apply_optimize_proposal(&pool, "fsrs", Utc::now())
                    .await
                    .is_err()
            );
        },
    )
    .await;
}
