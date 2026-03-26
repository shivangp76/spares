use super::data::CardData;
use crate::parsers::NotePart;
use crate::{CardErrorKind, LibraryError};

pub fn validate_cards(cards: &[CardData]) -> Result<(), LibraryError> {
    if cards.iter().any(|cd| cd.data.is_empty()) {
        return Err(LibraryError::Card(CardErrorKind::Empty));
    }
    if cards.iter().any(|cd| {
        !cd.data
            .iter()
            .any(|p| matches!(p, NotePart::SurroundingData(_)))
    }) {
        return Err(LibraryError::Card(CardErrorKind::MissingField(
            "Surrounding data".to_string(),
        )));
    }

    if cards.iter().any(|cd| {
        !cd.data
            .iter()
            .any(|p| matches!(p, NotePart::ImageOcclusion { .. }))
            && !cd
                .data
                .iter()
                .any(|p| matches!(p, NotePart::ClozeData(_, _)))
    }) {
        return Err(LibraryError::Card(CardErrorKind::MissingField(
            "Cloze or Image Occlusion".to_string(),
        )));
    }
    Ok(())
}
