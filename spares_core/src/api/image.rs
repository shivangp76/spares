//! Images pasted or dropped into a note in the browser.
//!
//! Notes refer to images by absolute path, so the browser cannot hand the server files of its own.
//! Instead, it uploads the image, which is stored in the image directory, and inserts the
//! returned snippet, which refers to the stored file.
//!
//! Images dragged from a web page are often not files the browser can hand over, only the
//! image's URL, which the server downloads instead (see [`create_image_from_url`]).
//!
//! Unlike image occlusions, nothing is ever written into a stored image, so files are named by
//! their contents: uploading the same image again reuses its file.

use std::fs::create_dir_all;
use std::fs::rename;
use std::fs::write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::api::image_occlusion::HASH_LENGTH;
use crate::api::image_occlusion::image_extension;
use crate::api::image_occlusion::other_error;
use crate::api::image_occlusion::sanitized_stem;
use crate::api::parser::get_parser_name;
use crate::config::get_data_dir;
use crate::parsers::Parseable;
use crate::parsers::find_parser;

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateImageResponse {
    /// The stored image.
    pub path: PathBuf,
    /// The image in the parser's syntax, ready to insert into a note.
    pub snippet: String,
}

/// Where images uploaded for notes are stored.
pub fn get_image_directory() -> PathBuf {
    get_data_dir().join("images")
}

/// Stores `image` in the image directory and returns its path. `image_filename` is only used for
/// the stem and extension of the stored name.
fn store_image(image: &[u8], image_filename: &str) -> Result<PathBuf, Error> {
    let extension = image_extension(image_filename)?;
    let stem = sanitized_stem(image_filename, "image");
    let hash = blake3::hash(image).to_hex();
    let directory = get_image_directory();
    create_dir_all(&directory).map_err(|e| Error::Io {
        description: format!("Failed to create {}", directory.display()),
        source: e,
    })?;
    let path = directory.join(format!("{stem}-{}.{extension}", &hash[..HASH_LENGTH]));
    if !path.exists() {
        // Write then rename so a partially written image is never rendered
        let temp_path = path.with_extension(format!("{extension}.tmp"));
        write(&temp_path, image)
            .and_then(|()| rename(&temp_path, &path))
            .map_err(|e| Error::Io {
                description: format!("Failed to write {}", path.display()),
                source: e,
            })?;
    }
    Ok(path)
}

/// Stores an image and returns the snippet to insert into a note with the given parser.
pub async fn create_image(
    db: &SqlitePool,
    parser_id: i64,
    image: &[u8],
    image_filename: &str,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<CreateImageResponse, Error> {
    let parser = find_parser(&get_parser_name(db, parser_id).await?, all_parsers)?;
    let path = store_image(image, image_filename)?;
    let snippet = parser.construct_image(&path);
    Ok(CreateImageResponse { path, snippet })
}

/// The largest image downloaded by [`create_image_from_url`], as for uploads.
pub const MAX_IMAGE_SIZE: usize = 64 * 1024 * 1024;

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// The extension of images of `content_type`, if cards can show them.
fn content_type_extension(content_type: &str) -> Option<&'static str> {
    let essence = content_type.split(';').next().unwrap_or_default().trim();
    match essence.to_lowercase().as_str() {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/bmp" => Some("bmp"),
        "image/svg+xml" => Some("svg"),
        _ => None,
    }
}

/// The name to store an image downloaded from `url` under: the URL's file name, with the extension
/// of `content_type` if the name has no supported one.
fn downloaded_image_filename(url: &reqwest::Url, content_type: Option<&str>) -> String {
    let name = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or_default();
    if image_extension(name).is_ok() {
        return name.to_string();
    }
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let extension = content_type
        .and_then(content_type_extension)
        .unwrap_or_default();
    format!("{stem}.{extension}")
}

/// Downloads the image at `url`, an http or https URL, and stores it as [`create_image`] does.
pub async fn create_image_from_url(
    db: &SqlitePool,
    parser_id: i64,
    url: &str,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<CreateImageResponse, Error> {
    let url =
        reqwest::Url::parse(url).map_err(|e| other_error(format!("Invalid URL {url}: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(other_error(format!(
            "Only http and https images can be downloaded, not {url}"
        )));
    }
    let too_large = || {
        other_error(format!(
            "{url} is larger than {} MB",
            MAX_IMAGE_SIZE / 1024 / 1024
        ))
    };
    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()?;
    let mut response = client.get(url.clone()).send().await?.error_for_status()?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    // Not an error page or other file named like an image
    if content_type
        .as_deref()
        .is_some_and(|t| !t.trim_start().to_lowercase().starts_with("image/"))
    {
        return Err(other_error(format!(
            "{url} is not an image, but {}",
            content_type.unwrap_or_default()
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_IMAGE_SIZE as u64)
    {
        return Err(too_large());
    }
    let mut image = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        image.extend_from_slice(&chunk);
        if image.len() > MAX_IMAGE_SIZE {
            return Err(too_large());
        }
    }
    let filename = downloaded_image_filename(&url, content_type.as_deref());
    create_image(db, parser_id, &image, &filename, all_parsers).await
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::Map;

    use super::*;
    use crate::api::note::create_notes;
    use crate::api::parser::tests::create_parser_helper;
    use crate::parsers::get_all_parsers;
    use crate::schema::note::CreateNoteRequest;
    use crate::schema::note::CreateNotesRequest;

    const IMAGE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"/>"#;

    #[test]
    fn test_store_names_images_by_contents() {
        let path = store_image(IMAGE, "Pasted image!.SVG").unwrap();
        assert_eq!(path.parent().unwrap(), get_image_directory());
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("Pasted_image_-"), "{name}");
        assert_eq!(path.extension().unwrap(), "svg");
        assert_eq!(std::fs::read(&path).unwrap(), IMAGE);

        // The same image is stored once, even when uploaded again under its stored name
        assert_eq!(store_image(IMAGE, "Pasted image!.svg").unwrap(), path);
        assert_eq!(store_image(IMAGE, name).unwrap(), path);
        // A different image gets its own file
        let other = br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"/>"#;
        assert_ne!(store_image(other, "Pasted image!.svg").unwrap(), path);
    }

    #[test]
    fn test_downloaded_image_filename() {
        let name = |url: &str, content_type: Option<&str>| {
            downloaded_image_filename(&reqwest::Url::parse(url).unwrap(), content_type)
        };
        assert_eq!(name("https://a.org/img/cell.PNG?w=2", None), "cell.PNG");
        assert_eq!(
            name("https://a.org/render/cell?id=1", Some("image/jpeg; q=1")),
            "cell.jpg"
        );
        assert_eq!(
            name("https://a.org/photo.jfif", Some("image/jpeg")),
            "photo.jpg"
        );
        assert_eq!(name("https://a.org/", Some("image/webp")), ".webp");
        // Left for `store_image` to reject
        assert_eq!(
            name("https://a.org/photo.heic", Some("image/heic")),
            "photo."
        );
    }

    #[sqlx::test]
    async fn test_create_image_from_url_rejects_other_schemes(pool: SqlitePool) -> () {
        let parser = create_parser_helper(&pool, "markdown").await;
        for url in [
            "file:///etc/passwd",
            "data:image/png;base64,AA==",
            "not a url",
        ] {
            assert!(
                create_image_from_url(&pool, parser.id, url, &get_all_parsers())
                    .await
                    .is_err(),
                "{url}"
            );
        }
    }

    #[test]
    fn test_store_rejects_unsupported_formats() {
        assert!(store_image(b"text", "notes.txt").is_err());
        assert!(store_image(IMAGE, "no_extension").is_err());
    }

    #[sqlx::test]
    async fn test_snippet_creates_a_note_for_every_parser(pool: SqlitePool) -> () {
        for parser_name in ["markdown", "typst", "latex-note"] {
            let parser = create_parser_helper(&pool, parser_name).await;
            let created = create_image(&pool, parser.id, IMAGE, "a.svg", &get_all_parsers())
                .await
                .unwrap();
            assert!(
                created.snippet.contains(created.path.to_str().unwrap()),
                "{parser_name}: {}",
                created.snippet
            );
            let (cloze_start, cloze_end) = find_parser(parser_name, &get_all_parsers())
                .unwrap()
                .construct_cloze("");
            create_notes(
                &pool,
                CreateNotesRequest {
                    parser_id: parser.id,
                    requests: vec![CreateNoteRequest {
                        data: format!("{cloze_start}A{cloze_end} next to\n{}\n", created.snippet),
                        keywords: Vec::new(),
                        tags: Vec::new(),
                        is_suspended: false,
                        custom_data: Map::new(),
                    }],
                },
                Utc::now(),
                &get_all_parsers(),
                true,
            )
            .await
            .unwrap();
        }
    }
}
