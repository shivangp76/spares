//! Tagging the cards you keep burying (see [`TagBuriedConfig`]).
//!
//! A card you bury yourself again and again, rather than rating it, is usually one to rewrite,
//! split or drop. Each card counts the days it was buried since it was last rated, in its custom
//! data under [`BURY_STREAK_KEY`]: burying it again on the same day doesn't count, and rating it
//! starts the count again. Once the count reaches [`TagBuriedConfig::days`], the card gets
//! [`TagBuriedConfig::tag`] as a tag of its own (see [`crate::api::card_tags`]), so `tag=buried`
//! finds them. The tag stays until you take it.
//!
//! Only burying a card yourself counts: Bury Card and Bury Note, and `PATCH /api/cards` with
//! `Buried`. The siblings burying buries, and cards buried until later in the day, don't.
//! Keeping the count in custom data means a sync merges it with the card, and undoing a bury
//! takes it back.

use chrono::DateTime;
use chrono::NaiveDate;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sqlx::sqlite::SqliteConnection;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::api::card_tags;
use crate::config::SparesExternalConfig;
use crate::config::TagBuriedConfig;
use crate::helpers::study_date;
use crate::model::CardId;
use crate::model::NoteId;
use crate::notes_vcs;
use crate::schema::note::UpdateTags;

/// The key of a card's custom data that counts the days it was buried since it was last rated
pub const BURY_STREAK_KEY: &str = "bury_streak";

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct BuryStreak {
    /// The last day of study it was buried on
    day: NaiveDate,
    /// The days it was buried on since it was last rated
    days: u32,
}

/// Counts a bury of a card at `at` in its `custom_data`. Returns whether the card has now been
/// buried on [`TagBuriedConfig::days`] days since it was last rated, so it should be tagged. Does
/// nothing if tagging buried cards is off.
pub(crate) fn count_bury(
    custom_data: &mut Value,
    at: DateTime<Utc>,
    config: &SparesExternalConfig,
) -> bool {
    let TagBuriedConfig { days, .. } = &config.queue.tag_buried;
    if *days == 0 {
        return false;
    }
    let Value::Object(map) = custom_data else {
        return false;
    };
    let today = study_date(at, config.zone());
    let streak = map
        .get(BURY_STREAK_KEY)
        .and_then(|streak| serde_json::from_value::<BuryStreak>(streak.clone()).ok());
    let streak = match streak {
        Some(streak) if streak.day >= today => streak,
        Some(streak) => BuryStreak {
            day: today,
            days: streak.days + 1,
        },
        None => BuryStreak {
            day: today,
            days: 1,
        },
    };
    let reached = streak.days >= *days;
    map.insert(
        BURY_STREAK_KEY.to_string(),
        serde_json::to_value(streak).expect("serializable"),
    );
    reached
}

/// Starts the count of a card's buries again, as rating it does
pub(crate) fn reset_bury_count(custom_data: &mut Value) {
    if let Value::Object(map) = custom_data {
        map.remove(BURY_STREAK_KEY);
    }
}

/// Gives `card_ids` [`TagBuriedConfig::tag`], logged in the group of the bury `group` if `log`.
/// Returns the event that leads the group, if one was logged.
pub(crate) async fn tag_buried_cards(
    db: &SqlitePool,
    card_ids: &[CardId],
    config: &SparesExternalConfig,
    at: DateTime<Utc>,
    log: bool,
    group: Option<i64>,
) -> Result<Option<i64>, Error> {
    if card_ids.is_empty() {
        return Ok(group);
    }
    let mut tx = db.begin().await.map_err(|e| Error::Sqlx { source: e })?;
    let (event_id, note_ids) =
        tag_buried_cards_on(&mut tx, card_ids, config, at, log, group).await?;
    tx.commit().await.map_err(|e| Error::Sqlx { source: e })?;
    notes_vcs::reconcile_after(db, note_ids, "tag buried cards").await;
    Ok(event_id)
}

/// [`tag_buried_cards`] in the caller's transaction. Returns the event that leads the group, and
/// the notes whose files the notes repository has to catch up with once it commits.
pub(crate) async fn tag_buried_cards_on(
    conn: &mut SqliteConnection,
    card_ids: &[CardId],
    config: &SparesExternalConfig,
    at: DateTime<Utc>,
    log: bool,
    group: Option<i64>,
) -> Result<(Option<i64>, Vec<NoteId>), Error> {
    if card_ids.is_empty() {
        return Ok((group, Vec::new()));
    }
    let update = UpdateTags::ModifyTags {
        tags_to_remove: None,
        tags_to_add: Some(vec![config.queue.tag_buried.tag.clone()]),
    };
    let changes = card_tags::update_card_tags_on(&mut *conn, card_ids, &update).await?;
    let note_ids: Vec<NoteId> = changes.note_ids.iter().copied().collect();
    let event_id = if log {
        card_tags::log_card_tag_changes_on(conn, changes, at, group)
            .await?
            .or(group)
    } else {
        group
    };
    Ok((event_id, note_ids))
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    fn config(days: u32) -> SparesExternalConfig {
        let mut config = SparesExternalConfig::default();
        config.queue.tag_buried.days = days;
        config.time_zone = Some("UTC".to_string());
        config
    }

    /// Noon of day `day` of November 2026, in UTC
    fn noon(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, day, 12, 0, 0).unwrap()
    }

    #[test]
    fn days_buried_count_until_rated() {
        let config = config(2);
        let mut custom_data = json!({"cloze_uid": "abc"});
        assert!(!count_bury(&mut custom_data, noon(1), &config));
        // Burying it again the same day is not another day
        assert!(!count_bury(
            &mut custom_data,
            noon(1) + Duration::hours(3),
            &config
        ));
        // A day without burying it in between still counts, as no rating came between
        assert!(count_bury(&mut custom_data, noon(3), &config));
        assert_eq!(
            custom_data["bury_streak"],
            json!({"day": "2026-11-03", "days": 2})
        );
        assert_eq!(custom_data["cloze_uid"], "abc");

        reset_bury_count(&mut custom_data);
        assert_eq!(custom_data, json!({"cloze_uid": "abc"}));
        assert!(!count_bury(&mut custom_data, noon(4), &config));
    }

    #[test]
    fn a_day_starts_at_day_starts_at() {
        let config = config(2);
        let mut custom_data = json!({});
        assert!(!count_bury(&mut custom_data, noon(1), &config));
        // 1 AM still belongs to the day before (`day_starts_at` is 4 AM by default)
        let one_am = Utc.with_ymd_and_hms(2026, 11, 2, 1, 0, 0).unwrap();
        assert!(!count_bury(&mut custom_data, one_am, &config));
        assert!(count_bury(&mut custom_data, noon(2), &config));
    }

    #[test]
    fn nothing_is_counted_when_off() {
        let config = config(0);
        let mut custom_data = json!({});
        assert!(!count_bury(&mut custom_data, noon(1), &config));
        assert!(!count_bury(&mut custom_data, noon(2), &config));
        assert_eq!(custom_data, json!({}));
    }
}
