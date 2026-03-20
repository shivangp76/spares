use super::{BackReveal, BackType, FrontConceal};
use crate::parsers::{ClozeGrouping, ClozeHiddenReplacement, NotePart, ReadableCardIdentifier};

#[derive(Clone, Debug, PartialEq)]
pub struct CardData {
    pub order: Option<usize>,
    pub previous_order: Option<usize>,
    pub grouping: ClozeGrouping,
    pub is_suspended: Option<bool>,
    pub front_conceal: FrontConceal,
    pub back_reveal: BackReveal,
    pub back_emphasis: bool,
    pub back_type: BackType,
    pub inherit: Option<ReadableCardIdentifier>,
    pub data: Vec<NotePart>,
}

impl CardData {
    /// Returns `true` if this card is a reverse card (i.e. created with `ro:` or as the backward
    /// direction of `r:`). Detected by finding a `ClozeData` part that appears *outside* of
    /// `ClozeStart`/`ClozeEnd` delimiters — forward cards only ever put `SurroundingData` there.
    pub fn is_reverse(&self) -> bool {
        let mut in_cloze = false;
        for part in &self.data {
            match part {
                NotePart::ClozeStart(_) => in_cloze = true,
                NotePart::ClozeEnd(_) => in_cloze = false,
                NotePart::ClozeData(_, _) if !in_cloze => return true,
                _ => {}
            }
        }
        false
    }
}
