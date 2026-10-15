//! Audio clips recorded in the browser and inserted into a note.
//!
//! As for [images](crate::api::image), notes refer to clips by absolute path, so the browser
//! uploads the recording, which is stored in the audio directory, and inserts the returned
//! snippet, which links to the stored file. Files are named by their contents.

use std::fs::create_dir_all;
use std::fs::rename;
use std::fs::write;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use sqlx::sqlite::SqlitePool;

use crate::Error;
use crate::api::image_occlusion::HASH_LENGTH;
use crate::api::image_occlusion::other_error;
use crate::api::image_occlusion::sanitized_stem;
use crate::api::parser::get_parser_name;
use crate::config::get_data_dir;
use crate::parsers::Parseable;
use crate::parsers::audio::AUDIO_EXTENSIONS;
use crate::parsers::audio::is_audio_path;
use crate::parsers::find_parser;

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateAudioResponse {
    /// The stored clip.
    pub path: PathBuf,
    /// A link to the clip in the parser's syntax, ready to insert into a note.
    pub snippet: String,
}

/// The largest clip accepted. Recordings are short, but uncompressed ones are large.
pub const MAX_AUDIO_SIZE: usize = 64 * 1024 * 1024;

/// Where clips recorded for notes are stored.
pub fn get_audio_directory() -> PathBuf {
    get_data_dir().join("audio")
}

/// Stores `audio` in the audio directory and returns its path. `audio_filename` is only used for
/// the stem and extension of the stored name.
fn store_audio(audio: &[u8], audio_filename: &str) -> Result<PathBuf, Error> {
    if !is_audio_path(audio_filename) {
        return Err(other_error(format!(
            "Unsupported audio: {audio_filename}. Supported formats: {}.",
            AUDIO_EXTENSIONS.join(", ")
        )));
    }
    // Checked above
    let extension = Path::new(audio_filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap()
        .to_lowercase();
    let stem = sanitized_stem(audio_filename, "audio");
    let hash = blake3::hash(audio).to_hex();
    let directory = get_audio_directory();
    create_dir_all(&directory).map_err(|e| Error::Io {
        description: format!("Failed to create {}", directory.display()),
        source: e,
    })?;
    let path = directory.join(format!("{stem}-{}.{extension}", &hash[..HASH_LENGTH]));
    if !path.exists() {
        // Write then rename so a partially written clip is never played
        let temp_path = path.with_extension(format!("{extension}.tmp"));
        write(&temp_path, audio)
            .and_then(|()| rename(&temp_path, &path))
            .map_err(|e| Error::Io {
                description: format!("Failed to write {}", path.display()),
                source: e,
            })?;
    }
    Ok(path)
}

/// Stores a clip and returns the snippet to insert into a note with the given parser.
pub async fn create_audio(
    db: &SqlitePool,
    parser_id: i64,
    audio: &[u8],
    audio_filename: &str,
    all_parsers: &[fn() -> Box<dyn Parseable>],
) -> Result<CreateAudioResponse, Error> {
    let parser_name = get_parser_name(db, parser_id).await?;
    let parser = find_parser(&parser_name, all_parsers)?;
    // Checked before storing, so a parser without clips leaves no file behind
    if parser.construct_audio(Path::new("/a.mp3")).is_none() {
        return Err(other_error(format!(
            "The {parser_name} parser has no audio clips"
        )));
    }
    let path = store_audio(audio, audio_filename)?;
    let snippet = parser.construct_audio(&path).unwrap();
    Ok(CreateAudioResponse { path, snippet })
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

    // Not a real recording, which nothing here decodes
    const AUDIO: &[u8] = b"RIFF\0\0\0\0WAVE";

    #[test]
    fn test_store_names_clips_by_contents() {
        let path = store_audio(AUDIO, "Recording 1.WAV").unwrap();
        assert_eq!(path.parent().unwrap(), get_audio_directory());
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("Recording_1-"), "{name}");
        assert_eq!(path.extension().unwrap(), "wav");
        assert_eq!(std::fs::read(&path).unwrap(), AUDIO);

        // The same clip is stored once, even when uploaded again under its stored name
        assert_eq!(store_audio(AUDIO, "Recording 1.wav").unwrap(), path);
        assert_eq!(store_audio(AUDIO, name).unwrap(), path);
        assert_ne!(store_audio(b"other", "Recording 1.wav").unwrap(), path);
    }

    #[test]
    fn test_store_rejects_unsupported_formats() {
        assert!(store_audio(AUDIO, "clip.webm").is_err());
        assert!(store_audio(AUDIO, "no_extension").is_err());
    }

    #[sqlx::test]
    async fn test_snippet_is_a_clip_for_every_parser_with_clips(pool: SqlitePool) -> () {
        for parser_name in ["markdown", "typst"] {
            let parser = create_parser_helper(&pool, parser_name).await;
            let created = create_audio(&pool, parser.id, AUDIO, "a.m4a", &get_all_parsers())
                .await
                .unwrap();
            let parseable = find_parser(parser_name, &get_all_parsers()).unwrap();
            // The whole snippet is the clip's link
            let refs = parseable.audio_refs(&created.snippet);
            assert_eq!(refs.len(), 1, "{parser_name}: {}", created.snippet);
            assert_eq!(refs[0].range, 0..created.snippet.len(), "{parser_name}");
            assert_eq!(
                refs[0].path,
                created.path.to_str().unwrap(),
                "{parser_name}"
            );

            let (cloze_start, cloze_end) = parseable.construct_cloze("");
            create_notes(
                &pool,
                CreateNotesRequest {
                    parser_id: parser.id,
                    requests: vec![CreateNoteRequest {
                        data: format!("{} {cloze_start}A{cloze_end}\n", created.snippet),
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

    #[sqlx::test]
    async fn test_parser_without_clips_is_rejected(pool: SqlitePool) -> () {
        let parser = create_parser_helper(&pool, "latex-note").await;
        let result = create_audio(&pool, parser.id, AUDIO, "a.m4a", &get_all_parsers()).await;
        assert!(result.is_err());
    }
}
