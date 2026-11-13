//! `spares vcs render-diff`: a page showing edited notes as source diff, old render and new
//! render (see `spares_core::render_diff`).
//!
//! jj runs it as a diff tool: `spares vcs init --jj` adds the alias `jj rd`, which is `jj diff
//! --tool spares-render-diff`, so it takes `jj diff`'s arguments (`jj rd -r 'main..@'`). jj
//! copies the changed files of each side into two directories, runs this with them, and deletes
//! them when it returns, so everything is read first and the page holds all it shows. With git:
//! `git difftool --dir-diff -x 'spares vcs render-diff'`.
//!
//! The renders come from the host (`POST /api/notes/vcs/render-preview`), which renders in its
//! sandbox with its Typst workers, so the server must be running, as it must be to push.

use std::path::Path;
use std::path::PathBuf;

use miette::Error;
use miette::miette;
use reqwest::Client;
use spares_core::render::PreviewResult;
use spares_core::render::PreviewSource;
use spares_core::render::RenderPreviewRequest;
use spares_core::render::RenderPreviewResponse;
use spares_core::render_diff::DiffSide;
use spares_core::render_diff::NoteDiff;
use spares_core::render_diff::NoteLanguage;
use spares_core::render_diff::PageInfo;
use spares_core::render_diff::note_changes;
use spares_core::render_diff::page_html;
use spares_core::render_diff::preview_name;

use crate::utils::ensure_ok;

/// Bytes of JSON per render request, below the host's limit (8 MiB) with room to spare
const BATCH_BYTES: usize = 6 * 1024 * 1024;

/// What `spares vcs render-diff` did
#[derive(Debug)]
pub(crate) struct RenderDiffOutcome {
    pub(crate) page: PathBuf,
    pub(crate) notes: Vec<NoteDiff>,
}

/// `spares vcs render-diff`: writes the page, prints its path and opens it. Notes that don't
/// compile are on the page and listed on standard error, but don't fail the command, which jj
/// would take for the tool failing.
pub(crate) async fn render_diff_command(
    base_url: &str,
    client: &Client,
    left: &Path,
    right: &Path,
    out: Option<PathBuf>,
    open: bool,
) -> Result<(), Error> {
    let outcome = render_diff(base_url, client, left, right, out).await?;
    for note in outcome.notes.iter().filter(|note| note.fails()) {
        eprintln!("{} does not compile", note.change.path);
    }
    println!("{}", outcome.page.display());
    if open {
        crate::utils::open_file(&outcome.page);
    }
    Ok(())
}

/// Renders the notes that differ between `left` (old) and `right` (new) through the host, and
/// writes the page to `out`, or a new file in the cache
pub(crate) async fn render_diff(
    base_url: &str,
    client: &Client,
    left: &Path,
    right: &Path,
    out: Option<PathBuf>,
) -> Result<RenderDiffOutcome, Error> {
    for dir in [left, right] {
        // A side may be missing, as jj leaves out a side without files
        if dir.exists() && !dir.is_dir() {
            return Err(miette!("{} is not a directory", dir.display()));
        }
    }
    let changes = note_changes(left, right).map_err(|e| {
        miette!(
            "Failed to read {} and {}: {e}",
            left.display(),
            right.display()
        )
    })?;

    // Each Typst side under a name of its own
    let mut sources: Vec<PreviewSource> = Vec::new();
    let mut names: Vec<(Option<usize>, Option<usize>)> = Vec::new();
    let mut taken = std::collections::HashSet::new();
    for (i, change) in changes.iter().enumerate() {
        let mut add = |text: &Option<String>, side: DiffSide| {
            let text = text
                .as_ref()
                .filter(|_| change.language == NoteLanguage::Typst)?;
            let mut name = preview_name(&change.id, side);
            if !taken.insert(name.clone()) {
                name = preview_name(&format!("{}-{i}", change.id), side);
                taken.insert(name.clone());
            }
            sources.push(PreviewSource {
                name,
                text: text.clone(),
            });
            Some(sources.len() - 1)
        };
        names.push((
            add(&change.old, DiffSide::Old),
            add(&change.new, DiffSide::New),
        ));
    }
    let mut results = render(base_url, client, sources).await?;

    let notes: Vec<NoteDiff> = changes
        .into_iter()
        .zip(names)
        .map(|(change, (old, new))| {
            // Diagnostics name the note's file, not the copy the host rendered
            let mut take = |i: usize| {
                let mut result = std::mem::take(&mut results[i]);
                result.diagnostics = result
                    .diagnostics
                    .replace(&format!("{}.typ", result.name), &change.path);
                result
            };
            let (old_render, new_render) = (old.map(&mut take), new.map(&mut take));
            NoteDiff {
                change,
                old_render,
                new_render,
            }
        })
        .collect();
    let info = PageInfo {
        title: page_title(&notes),
        // jj names no revisions, only its temporary directories
        subtitle: format!(
            "{} → {}, {}",
            std::path::absolute(left)
                .unwrap_or_else(|_| left.to_path_buf())
                .display(),
            std::path::absolute(right)
                .unwrap_or_else(|_| right.to_path_buf())
                .display(),
            chrono::Local::now().format("%Y-%m-%d %H:%M"),
        ),
    };
    let html = page_html(&info, &notes);
    let page = match out {
        Some(out) => out,
        None => default_page_path(),
    };
    if let Some(dir) = page.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| miette!("Failed to create {}: {e}", dir.display()))?;
    }
    std::fs::write(&page, html).map_err(|e| miette!("Failed to write {}: {e}", page.display()))?;
    Ok(RenderDiffOutcome { page, notes })
}

/// "Render diff: 3 notes" or the like
fn page_title(notes: &[NoteDiff]) -> String {
    match notes {
        [] => "Render diff: no notes changed".to_string(),
        [note] => format!("Render diff: {}", note.change.id),
        notes => format!("Render diff: {} notes", notes.len()),
    }
}

/// Renders `sources` through the host, in requests below its body limit. One result per source,
/// in order.
async fn render(
    base_url: &str,
    client: &Client,
    sources: Vec<PreviewSource>,
) -> Result<Vec<PreviewResult>, Error> {
    let mut results = Vec::with_capacity(sources.len());
    for batch in batches(sources) {
        let size: usize = batch.iter().map(json_size).sum();
        if batch.len() == 1 && size > BATCH_BYTES {
            results.push(PreviewResult {
                name: batch[0].name.clone(),
                diagnostics: format!("error: too large to render here ({size} bytes)\n"),
                ..PreviewResult::default()
            });
            continue;
        }
        let response = client
            .post(format!("{base_url}/api/notes/vcs/render-preview"))
            .json(&RenderPreviewRequest { sources: batch })
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    miette!(
                        "Couldn't reach spares at {base_url}: start spares_server; render-diff renders through it"
                    )
                } else {
                    miette!("{e}")
                }
            })?;
        let response: RenderPreviewResponse = ensure_ok(response)
            .await?
            .json()
            .await
            .map_err(|e| miette!("{e}"))?;
        results.extend(response.results);
    }
    Ok(results)
}

/// About how many bytes `source` takes in a request
fn json_size(source: &PreviewSource) -> usize {
    serde_json::to_string(source).map_or(source.text.len() * 2, |json| json.len()) + 1
}

/// `sources` in order, split so each batch's JSON is at most [`BATCH_BYTES`] (or one source)
fn batches(sources: Vec<PreviewSource>) -> Vec<Vec<PreviewSource>> {
    let mut batches: Vec<Vec<PreviewSource>> = Vec::new();
    let mut size = 0;
    for source in sources {
        let source_size = json_size(&source);
        match batches.last_mut() {
            Some(batch) if size + source_size <= BATCH_BYTES => {
                size += source_size;
                batch.push(source);
            }
            _ => {
                size = source_size;
                batches.push(vec![source]);
            }
        }
    }
    batches
}

/// A new file for the page: `tmp/render-diff/<time>.html` in this machine's collection's cache,
/// else in the system's temporary directory
fn default_page_path() -> PathBuf {
    let dir = spares_core::collection::current().map_or_else(
        || std::env::temp_dir().join("spares-render-diff"),
        |collection| collection.paths.cache.join("tmp").join("render-diff"),
    );
    let time = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut path = dir.join(format!("{time}.html"));
    for n in 2.. {
        if !path.exists() {
            break;
        }
        path = dir.join(format!("{time}-{n}.html"));
    }
    path
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use spares_server::AppState;
    use spares_server::host::Host;
    use spares_server::host::accounts::NewUser;
    use spares_server::route::create_router;

    use super::*;

    fn source(name: &str, len: usize) -> PreviewSource {
        PreviewSource {
            name: name.to_string(),
            text: "x".repeat(len),
        }
    }

    #[test]
    fn batches_stay_under_the_limit() {
        let half = BATCH_BYTES / 2 - 100;
        let names = |batches: &[Vec<PreviewSource>]| -> Vec<Vec<String>> {
            batches
                .iter()
                .map(|batch| batch.iter().map(|s| s.name.clone()).collect())
                .collect()
        };
        let split = batches(vec![
            source("a", half),
            source("b", half),
            source("c", half),
            source("big", BATCH_BYTES * 2),
            source("d", 10),
        ]);
        assert_eq!(
            names(&split),
            [vec!["a", "b"], vec!["c"], vec!["big"], vec!["d"]]
        );
        assert_eq!(batches(Vec::new()).len(), 0);
    }

    fn note(body: &str) -> String {
        format!(
            "#let se(body) = none\n#let key(body) = body\n#let cl(..args) = args.pos().first()\n// spares: start\n#se[note-id: x]\n// spares: note start\n[#key[A note]]\n\n{body}\n// spares: note end\n\n// spares: end\n"
        )
    }

    /// Against a scratch host on a port of its own: one note modified, one added, one deleted,
    /// and one that no longer compiles give a page with a section for each, in that order of
    /// importance, with their badges.
    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::too_many_lines, reason = "one scenario")]
    async fn renders_a_page_through_a_scratch_host() {
        let root = std::env::temp_dir().join(format!(
            "spares-render-diff-cli-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        // A new root, so no `[sync]` and no other host is involved
        let host = Host::create(&root.join("host"), "127.0.0.1:0")
            .await
            .unwrap();
        host.create_user(NewUser {
            id: Some("me"),
            name: "me",
            email: None,
            password: "password1",
            admin: true,
            email_verified: true,
        })
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let router = create_router(
            Arc::new(AppState {
                host: Arc::clone(&host),
            }),
            None,
        );
        tokio::spawn(async move { axum::serve(listener, router).await });

        let (left, right) = (root.join("left"), root.join("right"));
        for dir in [&left, &right] {
            std::fs::create_dir_all(dir.join("typst")).unwrap();
        }
        let write = |dir: &Path, name: &str, text: &str| {
            std::fs::write(dir.join("typst").join(name), text).unwrap();
        };
        write(&left, "mod.typ", &note("#cl[the quick fox][o:1]"));
        write(
            &right,
            "mod.typ",
            &note("#cl[the slow fox][o:1]\n\nNext page:\n#pagebreak()\nMore."),
        );
        write(&right, "add.typ", &note("A new note."));
        write(&left, "del.typ", &note("An old note."));
        write(&left, "bad.typ", &note("Fine."));
        write(&right, "bad.typ", &note("#unknown-function()"));
        std::fs::write(left.join("ignored.txt"), "not a note").unwrap();

        // Without a token
        let refused = render_diff(
            &base_url,
            &Client::new(),
            &left,
            &right,
            Some(root.join("x.html")),
        )
        .await;
        assert!(refused.is_err());
        assert!(!root.join("x.html").exists());

        let token: serde_json::Value = Client::new()
            .post(format!("{base_url}/api/auth/token"))
            .json(&serde_json::json!({ "user": "me", "password": "password1", "name": "test" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", token["token"].as_str().unwrap())
                .parse()
                .unwrap(),
        );
        let client = Client::builder().default_headers(headers).build().unwrap();
        let page = root.join("out/page.html");
        let outcome = render_diff(&base_url, &client, &left, &right, Some(page.clone()))
            .await
            .unwrap();
        assert_eq!(outcome.page, page);
        let html = std::fs::read_to_string(&page).unwrap();
        for (i, id) in ["bad", "mod", "add", "del"].iter().enumerate() {
            assert!(
                html.contains(&format!("<a class=\"id\" href=\"#n{i}\">{id}</a>")),
                "section {i} isn't {id}"
            );
        }
        assert!(html.contains("compile error (new)"));
        assert!(html.contains("unknown variable: unknown-function"));
        assert!(html.contains("┌─ typst/bad.typ:"), "{html}");
        assert!(html.contains("<span class=\"badge added\">1 added</span>"));
        assert!(html.contains("<span class=\"badge deleted\">1 deleted</span>"));
        assert!(html.contains("<span class=\"badge modified\">2 modified</span>"));
        assert!(html.contains("<span class=\"badge error\">1 failed to compile</span>"));
        assert!(html.contains("<span class=\"em\">slow</span>"));
        assert!(!html.contains("ignored.txt"));
        let modified = outcome.notes.iter().find(|n| n.change.id == "mod").unwrap();
        assert_eq!(modified.old_render.as_ref().unwrap().pages.len(), 1);
        assert_eq!(modified.new_render.as_ref().unwrap().pages.len(), 2);
        assert_eq!(html.matches("<img alt=").count(), 1 + 2 + 1 + 1 + 1);

        // Nothing written to the collection: its files directory has no renders
        let files = root.join("host/cache/users/me/files");
        assert_eq!(
            std::fs::read_dir(&files).unwrap().count(),
            0,
            "{}",
            files.display()
        );

        // No server
        drop(host);
        let unreachable = render_diff(
            "http://127.0.0.1:9",
            &client,
            &left,
            &right,
            Some(root.join("y.html")),
        )
        .await;
        let message = format!("{:?}", unreachable.unwrap_err());
        assert!(message.contains("start spares_server"), "{message}");
        assert!(!root.join("y.html").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
