pub mod card;
pub mod note;
pub mod parser;
pub mod review;
pub mod scheduler;
pub mod statistics;
pub mod tag;
#[cfg(test)]
pub(crate) mod tests;

pub use card::{
    create_card_tags, delete_card_tags, forget_card, get_card, get_cards, get_leeches, update_card,
};

pub(crate) fn get_placeholders(length: usize) -> String {
    std::iter::repeat_n("?", length)
        .collect::<Vec<&str>>()
        .join(", ")
}
