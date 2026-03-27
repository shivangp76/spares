#[cfg(test)]
pub const DEFAULT_BACK_EMPHASIS: bool = false;

#[cfg(not(test))]
pub const DEFAULT_BACK_EMPHASIS: bool = true;

mod card_settings;
mod data;

pub use card_settings::*;
pub use data::*;
