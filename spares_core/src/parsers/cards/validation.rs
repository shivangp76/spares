use super::data::CardData;
use crate::CardErrorKind;
use crate::LibraryError;
use crate::parsers::NotePart;

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
            && !cd.data.iter().any(|p| matches!(p, NotePart::Cli { .. }))
    }) {
        return Err(LibraryError::Card(CardErrorKind::MissingField(
            "Cloze, Image Occlusion, or Cli".to_string(),
        )));
    }
    Ok(())
}
