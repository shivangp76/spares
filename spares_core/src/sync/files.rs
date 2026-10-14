//! Files after a sync (section 7 of `docs/sync-plan.md`). The database is the source of truth, so
//! the files of every note a sync changed are regenerated from it, and those of deleted notes
//! removed, on whichever side the change landed.
//!
//! With the notes repository ([`notes_vcs`]), regenerated note files are committed to the server
//! copy, and edits not pushed yet live in the user's clone, so a sync never overwrites them: they
//! meet the sync's commit when the clone is rebased.

use std::collections::BTreeSet;
use std::path::Path;

use log::error;
use log::info;
use sqlx::SqlitePool;

use super::bump_notes_version;
use super::http::HttpRemote;
use super::run::SyncMode;
use super::run::SyncReport;
use super::run::sync;
use crate::Error;
use crate::api::note::delete_card_files;
use crate::api::note::delete_note_files;
use crate::api::note::render_notes;
use crate::config::read_external_config;
use crate::model::NoteId;
use crate::notes_vcs;
use crate::parsers::Parseable;
use crate::parsers::RenderOutputDirectoryType;
use crate::parsers::find_parser;
use crate::parsers::generate_files::CardSide;
use crate::parsers::generate_files::RenderOutputType;
use crate::parsers::get_all_parsers;
use crate::parsers::get_output_raw_dir;
use crate::schema::note::NotesSelector;
use crate::schema::note::RenderNotesRequest;

/// Syncs with `sync_url` from the config, then updates files. For the client.
pub async fn sync_now(
    db: &SqlitePool,
    mode: SyncMode,
    temp_dir: &Path,
) -> Result<SyncReport, Error> {
    let url = read_external_config()?
        .sync_url
        .ok_or_else(|| Error::Sync("`sync_url` is not set in config.toml".to_string()))?;
    let remote = HttpRemote::new(&url)?;
    let report = sync(db, &remote, mode, temp_dir).await?;
    after_sync(db, &report.notes_changed, &report.notes_deleted).await;
    info!(
        "Synced with {url}: {} received, {} sent, {} conflicts",
        report.pulled,
        report.pushed,
        report.conflicts.len()
    );
    Ok(report)
}

/// Every note id, for telling afterwards which notes a full sync removed.
pub async fn note_ids(db: &SqlitePool) -> Result<BTreeSet<NoteId>, Error> {
    let ids: Vec<NoteId> = sqlx::query_scalar("SELECT id FROM note")
        .fetch_all(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
    Ok(ids.into_iter().collect())
}

/// Regenerates the files of `changed` notes and removes those of `deleted` ones, and card files
/// left over from cards a sync removed, and lets readers of the note change number know. The
/// database change has happened, so failures are logged rather than returned.
pub async fn after_sync(db: &SqlitePool, changed: &BTreeSet<NoteId>, deleted: &BTreeSet<NoteId>) {
    if changed.is_empty() && deleted.is_empty() {
        return;
    }
    let bumped = match db.acquire().await {
        Ok(mut conn) => bump_notes_version(&mut conn).await,
        Err(e) => Err(Error::Sqlx { source: e }),
    };
    if let Err(e) = bumped {
        error!("Failed to record that a sync changed notes: {e}");
    }
    if let Err(e) = update_files(db, changed, deleted).await {
        error!("Failed to update note files after a sync: {e}");
    }
}

async fn update_files(
    db: &SqlitePool,
    changed: &BTreeSet<NoteId>,
    deleted: &BTreeSet<NoteId>,
) -> Result<(), Error> {
    let parsers = get_all_parsers();
    for &note_id in changed.iter().chain(deleted) {
        let current: Option<(String, String, i64)> = sqlx::query_as(
            "SELECT p.name, n.data, (SELECT COUNT(*) FROM card WHERE note_id = n.id)
             FROM note n JOIN parser p ON p.id = n.parser_id WHERE n.id = ?",
        )
        .bind(note_id)
        .fetch_optional(db)
        .await
        .map_err(|e| Error::Sqlx { source: e })?;
        match current {
            Some((parser_name, data, card_count)) => {
                let parser = find_parser(&parser_name, &parsers)?;
                let card_count = usize::try_from(card_count).unwrap_or(0);
                let stale: Vec<usize> = card_orders_on_disk(parser.as_ref(), note_id)
                    .into_iter()
                    .filter(|order| *order > card_count)
                    .collect();
                let image_occlusions = parser
                    .get_image_occlusions(&data)
                    .map_or(0, |occlusions| occlusions.len());
                delete_card_files(parser.as_ref(), note_id, &stale, image_occlusions)?;
            }
            None => {
                // Its parser is gone with it, so every parser's files are checked.
                for parser in parsers.iter().map(|p| p()) {
                    let orders: Vec<usize> = card_orders_on_disk(parser.as_ref(), note_id)
                        .into_iter()
                        .collect();
                    delete_note_files(parser.as_ref(), note_id, &orders, "")?;
                }
            }
        }
    }
    if !deleted.is_empty() {
        notes_vcs::reconcile_after(db, deleted.iter().copied().collect(), "sync").await;
    }
    if !changed.is_empty() {
        render_notes(
            db,
            RenderNotesRequest {
                selector: NotesSelector::Ids(changed.iter().copied().collect()),
                immutable_note_ids: None,
                overridden_output_raw_dir: None,
                include_linked_notes: false,
                include_cards: true,
                generate_rendered: true,
                force_generate_rendered: false,
            },
            &parsers,
        )
        .await?;
    }
    Ok(())
}

/// The orders of the cards of `note_id` that have raw or rendered files.
fn card_orders_on_disk(parser: &dyn Parseable, note_id: NoteId) -> BTreeSet<usize> {
    let prefix = format!("{}-", note_id.letters());
    let dirs = [
        get_output_raw_dir(
            parser.get_parser_name(),
            RenderOutputType::Card(1, CardSide::Front),
            None,
        ),
        parser.get_output_rendered_dir(RenderOutputDirectoryType::Card),
    ];
    let mut orders = BTreeSet::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(rest) = name.to_str().and_then(|n| n.strip_prefix(&prefix)) else {
                continue;
            };
            let Some((order, side)) = rest.split_once('-') else {
                continue;
            };
            if !(side.starts_with("front") || side.starts_with("back")) {
                continue;
            }
            if let Ok(order) = order.parse() {
                orders.insert(order);
            }
        }
    }
    orders
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_card_orders_on_disk() {
        let parser = find_parser("markdown", &get_all_parsers()).unwrap();
        let note_id: NoteId = "zzq".parse().unwrap();
        let dir = get_output_raw_dir(
            parser.get_parser_name(),
            RenderOutputType::Card(1, CardSide::Front),
            None,
        );
        std::fs::create_dir_all(&dir).unwrap();
        let files = [
            "zzq-1-front.md",
            "zzq-1-back.md",
            "zzq-12-front.md",
            "zzqa-3-front.md",
        ];
        for file in files {
            std::fs::write(dir.join(file), "").unwrap();
        }
        assert_eq!(
            card_orders_on_disk(parser.as_ref(), note_id),
            BTreeSet::from([1, 12])
        );
        for file in files {
            std::fs::remove_file(dir.join(file)).unwrap();
        }
    }
}
