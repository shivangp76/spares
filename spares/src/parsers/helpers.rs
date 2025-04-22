use crate::parsers::RenderOutputType;
use crate::{config::get_data_dir, model::NoteId};
use std::path::{Path, PathBuf};

/// This cannot be overridden since [`get_note_info_from_filepath`] needs to be deterministic.
pub fn get_output_raw_dir(
    parser_name: &str,
    output_type: RenderOutputType,
    overridden_base_dir: Option<&Path>,
) -> PathBuf {
    let mut output_raw_dir = match overridden_base_dir {
        Some(base_dir) => base_dir.to_path_buf(),
        _ => get_data_dir(),
    };
    // Note/card is split before parser since everything in the `cards` subdirectory will never have to be edited. Only its rendered file will be viewed. This is not true for notes. Notes will have both their raw file and rendered file viewed. Thus, this will make it easier to just search through notes, instead of notes and cards.
    let sub_dir = match output_type {
        RenderOutputType::Note => "notes",
        RenderOutputType::Card(..) => "cards",
    };
    output_raw_dir.push(sub_dir);
    // SAFETY: The parser is validated so that its name is a valid directory name.
    output_raw_dir.push(parser_name);
    output_raw_dir
}

#[derive(Debug)]
pub struct NoteFilepathData {
    pub parser_name: String,
    pub note_id: NoteId,
}

pub fn get_note_info_from_filepath(note_filepath: &Path) -> Result<NoteFilepathData, String> {
    let parser_name = note_filepath
        .parent()
        .ok_or("Failed to get parent".to_string())?
        .file_name()
        .ok_or("Failed to get file name".to_string())?
        .to_str()
        .ok_or("Failed to convert to string".to_string())?
        .to_string();
    let note_id = note_filepath
        .file_stem()
        .ok_or("Failed to get file stem".to_string())?
        .to_str()
        .ok_or("Failed to convert to string".to_string())?
        .parse::<NoteId>()
        .map_err(|_| "Failed to parse note id".to_string())?;
    Ok(NoteFilepathData {
        parser_name,
        note_id,
    })
}
