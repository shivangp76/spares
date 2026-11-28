use std::sync::Arc;

use indoc::indoc;
use sqlx::SqlitePool;

use super::*;
use crate::api::undo::redo_event;
use crate::api::undo::undo_event;
use crate::collection::Collection;
use crate::collection::CollectionPaths;
use crate::config::read_external_config;
use crate::schema::undo::RedoEventRequest;
use crate::schema::undo::UndoEventRequest;

fn doc(text: &str) -> DocumentMut {
    text.parse().unwrap()
}

/// Runs `test` in a collection of its own, whose `config.toml` starts as `config` (or is missing).
async fn in_collection<F: std::future::Future<Output = ()>>(
    pool: &SqlitePool,
    config: Option<&str>,
    test: impl FnOnce(Arc<Collection>) -> F,
) {
    let root = tempfile::tempdir().unwrap();
    let paths = CollectionPaths::in_dir(root.path());
    std::fs::create_dir_all(&paths.config).unwrap();
    if let Some(config) = config {
        std::fs::write(paths.config_file(), config).unwrap();
    }
    let collection = Arc::new(Collection::new("settings", pool.clone(), paths));
    collection.scope(test(Arc::clone(&collection))).await;
    drop(root);
}

const FILE: &str = indoc! {r#"
    # My settings
    new_cards_daily_limit = 15 # a few a day
    audio_player = "mpv"

    [sync]
    url = "http://server:8080"
    replica = 0

    [queue]
    review_order = "retrievability"
"#};

#[test]
fn a_config_file_splits_into_this_machines_keys_and_the_collections() {
    let collection = collection_part(FILE).unwrap().to_string();
    assert!(
        collection.contains("new_cards_daily_limit = 15 # a few a day"),
        "{collection}"
    );
    assert!(collection.contains("[queue]"), "{collection}");
    assert!(!collection.contains("sync"), "{collection}");
    assert!(!collection.contains("audio_player"), "{collection}");
    let local = local_part(FILE).unwrap().to_string();
    assert!(local.contains("[sync]"), "{local}");
    assert!(local.contains("audio_player"), "{local}");
    assert!(!local.contains("queue"), "{local}");
}

#[test]
fn an_overlay_sets_values_not_whole_tables() {
    let base = indoc! {r#"
        # mine
        [queue]
        review_order = "due"
        new_order = "random"
    "#};
    let over = doc("[queue]\nreview_order = \"retrievability\"\n");
    let text = overlay(base, &over).unwrap();
    assert!(text.contains("# mine"), "{text}");
    assert!(text.contains("new_order = \"random\""), "{text}");
    assert!(text.contains("review_order = \"retrievability\""), "{text}");
    assert!(!text.contains("\"due\""), "{text}");
}

#[test]
fn set_item_makes_tables_and_writes_into_inline_tables() {
    let mut d = doc("queue = { review_order = \"due\" }\n");
    set_item(
        &mut d,
        &["queue".into(), "new_order".into()],
        Some(parse_value("\"random\"").unwrap()),
    );
    set_item(
        &mut d,
        &["leech".into(), "lapses_threshold".into()],
        Some(parse_value("4").unwrap()),
    );
    let config = crate::config::parse_external_config(&d.to_string()).unwrap();
    assert_eq!(config.queue.new_order, crate::config::NewCardOrder::Random);
    assert_eq!(config.leech.lapses_threshold, 4);
    // Removing the last key of a table removes the table
    set_item(&mut d, &["leech".into(), "lapses_threshold".into()], None);
    assert!(!d.to_string().contains("leech"), "{d}");
}

#[test]
fn a_merge_takes_each_sides_changes_key_by_key() {
    let base =
        "new_cards_daily_limit = 20\nreviews_daily_limit = 200\n\n[leech]\nlapses_threshold = 8\n";
    let local = "# here\nnew_cards_daily_limit = 10\nreviews_daily_limit = 200\n\n[leech]\nlapses_threshold = 8\n";
    let remote = "new_cards_daily_limit = 20\nreviews_daily_limit = 300\nload_balance = false\n";
    let (merged, conflicts) = merge3(base, local, remote, Prefer::Local).unwrap();
    assert_eq!(conflicts, Vec::<String>::new());
    assert!(
        merged.contains("# here"),
        "this side's comments stay: {merged}"
    );
    let config = crate::config::parse_external_config(&merged).unwrap();
    assert_eq!(config.new_cards_daily_limit, 10);
    assert_eq!(config.reviews_daily_limit, 300);
    assert!(!config.load_balance);
    // Removed over there, and the table with it
    assert!(!merged.contains("leech"), "{merged}");
}

#[test]
fn a_key_both_sides_changed_takes_the_preferred_side() {
    let base = "new_cards_daily_limit = 20\n";
    let local = "new_cards_daily_limit = 10\n";
    let remote = "new_cards_daily_limit = 30\n";
    let (merged, conflicts) = merge3(base, local, remote, Prefer::Local).unwrap();
    assert_eq!(conflicts, ["new_cards_daily_limit"]);
    assert_eq!(merged, local);
    let (merged, _) = merge3(base, local, remote, Prefer::Remote).unwrap();
    assert!(merged.contains("30"), "{merged}");
    // The same change on both sides is no conflict
    let (_, conflicts) = merge3(base, local, local, Prefer::Remote).unwrap();
    assert_eq!(conflicts, Vec::<String>::new());
}

#[test]
fn changed_keys_ignore_formatting() {
    let keys = changed_keys(
        "a_value = [1, 2]\n[queue]\nreview_order = \"due\"\n",
        "a_value = [ 1,2 ] # same\n[queue]\nreview_order = \"random\"\n",
    )
    .unwrap();
    assert_eq!(keys, ["queue.review_order"]);
}

#[sqlx::test]
async fn opening_moves_the_collections_settings_out_of_config_toml(pool: SqlitePool) {
    in_collection(&pool, Some(FILE), |collection| async move {
        let pool = collection.db.clone();
        // Before: config.toml alone
        assert_eq!(read_external_config().unwrap().new_cards_daily_limit, 15);
        load(&pool).await.unwrap();
        let file = std::fs::read_to_string(collection.paths.config_file()).unwrap();
        assert!(file.starts_with(MOVED_HEADER), "{file}");
        assert!(
            file.contains("[sync]") && file.contains("audio_player"),
            "{file}"
        );
        assert!(!file.contains("new_cards_daily_limit"), "{file}");
        let backup = std::fs::read_to_string(backup_path(&collection.paths.config_file())).unwrap();
        assert_eq!(backup, FILE);
        let mut conn = pool.acquire().await.unwrap();
        let text = read_text(&mut conn).await.unwrap().unwrap();
        assert!(text.contains("# a few a day"), "comments move too: {text}");
        let config = read_external_config().unwrap();
        assert_eq!(config.new_cards_daily_limit, 15);
        assert_eq!(config.audio_player, "mpv");
        assert_eq!(config.sync.replica, Some(0));

        // A second open changes nothing
        load(&pool).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(collection.paths.config_file()).unwrap(),
            file
        );

        // A collection key written into config.toml by hand applies at once, and moves in on the
        // next open
        std::fs::write(
            collection.paths.config_file(),
            format!("new_cards_daily_limit = 5\n{file}"),
        )
        .unwrap();
        assert_eq!(read_external_config().unwrap().new_cards_daily_limit, 5);
        load(&pool).await.unwrap();
        assert_eq!(read_external_config().unwrap().new_cards_daily_limit, 5);
        let text = read_text(&mut conn).await.unwrap().unwrap();
        assert!(text.contains("new_cards_daily_limit = 5"), "{text}");
        assert!(text.contains("[queue]"), "{text}");
        let file = std::fs::read_to_string(collection.paths.config_file()).unwrap();
        assert!(!file.contains("new_cards_daily_limit"), "{file}");
    })
    .await;
}

#[sqlx::test]
async fn an_invalid_config_toml_is_left_where_it_is(pool: SqlitePool) {
    let invalid = "new_cards_daily_limit = \"many\"\n";
    in_collection(&pool, Some(invalid), |collection| async move {
        let pool = collection.db.clone();
        load(&pool).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(collection.paths.config_file()).unwrap(),
            invalid
        );
        assert!(read_external_config().is_err());
    })
    .await;
}

#[sqlx::test]
async fn without_any_settings_the_defaults_apply_and_nothing_is_written(pool: SqlitePool) {
    in_collection(&pool, None, |collection| async move {
        let pool = collection.db.clone();
        load(&pool).await.unwrap();
        let config = read_external_config().unwrap();
        let defaults = crate::config::SparesExternalConfig::default();
        assert_eq!(config.new_cards_daily_limit, defaults.new_cards_daily_limit);
        assert_eq!(config.learning_steps, defaults.learning_steps);
        assert!(!collection.paths.config_file().exists());
        let mut conn = pool.acquire().await.unwrap();
        assert_eq!(read_text(&mut conn).await.unwrap(), None);
    })
    .await;
}

#[sqlx::test]
async fn a_change_is_checked_logged_and_undone(pool: SqlitePool) {
    Box::pin(in_collection(&pool, Some(FILE), |collection| async move {
        let pool = collection.db.clone();
        // This machine's settings stay in config.toml
        let error = set_values(
            &pool,
            &[(vec!["sync".into(), "replica".into()], Some("3".into()))],
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("this machine's"), "{error}");
        // An invalid value changes nothing
        let error = set_values(
            &pool,
            &[(vec!["new_cards_daily_limit".into()], Some("-1".into()))],
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("the settings line 2"), "{error}");
        assert_eq!(read_external_config().unwrap().new_cards_daily_limit, 15);

        let change = set_values(
            &pool,
            &[
                (vec!["new_cards_daily_limit".into()], Some("25".into())),
                (
                    vec!["leech".into(), "lapses_threshold".into()],
                    Some("4".into()),
                ),
            ],
        )
        .await
        .unwrap();
        assert!(change.event_id.is_some());
        let config = read_external_config().unwrap();
        assert_eq!(config.new_cards_daily_limit, 25);
        assert_eq!(config.leech.lapses_threshold, 4);
        assert!(change.after.contains("# a few a day"), "{}", change.after);

        // A change that changes nothing isn't logged
        let same = set_values(
            &pool,
            &[(vec!["new_cards_daily_limit".into()], Some("25".into()))],
        )
        .await
        .unwrap();
        assert!(!same.changed() && same.event_id.is_none());

        let undone = undo_event(
            &pool,
            UndoEventRequest {
                event_id: None,
                undo_group: false,
            },
        )
        .await
        .unwrap()
        .expect("undone");
        let description = &undone.undone_events[0].description;
        assert!(description.contains("settings"), "{description}");
        let config = read_external_config().unwrap();
        assert_eq!(config.new_cards_daily_limit, 15);
        assert_eq!(config.leech.lapses_threshold, 8);

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
        assert_eq!(read_external_config().unwrap().new_cards_daily_limit, 25);
    }))
    .await;
}

#[sqlx::test]
async fn a_change_is_not_undone_over_a_later_one(pool: SqlitePool) {
    in_collection(&pool, Some(FILE), |collection| async move {
        let pool = collection.db.clone();
        let first = set_values(
            &pool,
            &[(vec!["new_cards_daily_limit".into()], Some("25".into()))],
        )
        .await
        .unwrap();
        // Another key changed since doesn't stand in the way
        replace(
            &pool,
            &format!("{}reviews_daily_limit = 100\n", first.after),
        )
        .await
        .unwrap();
        set_values(
            &pool,
            &[(vec!["new_cards_daily_limit".into()], Some("30".into()))],
        )
        .await
        .unwrap();
        let error = undo_event(
            &pool,
            UndoEventRequest {
                event_id: first.event_id,
                undo_group: false,
            },
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("new_cards_daily_limit"),
            "{error}"
        );
        assert_eq!(read_external_config().unwrap().new_cards_daily_limit, 30);
    })
    .await;
}

#[test]
fn an_overlay_onto_nothing_keeps_every_comment() {
    let over = doc(indoc! {r#"
        # My settings
        load_balance = true # yes

        # How cards come
        [queue]
        # first the due ones
        review_order = "due"
    "#});
    assert_eq!(overlay("", &over).unwrap(), over.to_string());
}

#[test]
fn setting_a_value_keeps_the_comments_around_it() {
    let mut d = doc(
        "# My settings\nnew_cards_daily_limit = 15 # a few\n\n# order\n[queue]\nreview_order = \"due\"\n",
    );
    set_item(
        &mut d,
        &["new_cards_daily_limit".into()],
        Some(parse_value("25").unwrap()),
    );
    set_item(
        &mut d,
        &["queue".into(), "review_order".into()],
        Some(parse_value("\"random\"").unwrap()),
    );
    assert_eq!(
        d.to_string(),
        "# My settings\nnew_cards_daily_limit = 25 # a few\n\n# order\n[queue]\nreview_order = \"random\"\n"
    );
}
