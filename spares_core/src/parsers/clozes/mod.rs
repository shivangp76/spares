#[cfg(test)]
pub const DEFAULT_BACK_EMPHASIS: bool = false;

#[cfg(not(test))]
pub const DEFAULT_BACK_EMPHASIS: bool = true;

mod card_settings;
mod data;
mod matching;

pub use card_settings::*;
pub use data::*;
pub use matching::*;
