use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct OverlapperConfig {
    pub context_before_item: u32,
    /// The number of clozes that require an answer at once
    pub prompts: u32,
    pub context_after_item: u32,
    /// Useful when you need to know the exact starting point of a sequence
    pub no_cues_for_first_item: bool,
    /// Useful when you need to know the exact ending point of a sequence
    pub no_cues_for_last_item: bool,
    pub start_and_end_gradually: bool,
}

impl Default for OverlapperConfig {
    fn default() -> Self {
        Self {
            context_before_item: 1,
            prompts: 1,
            context_after_item: 0,
            no_cues_for_first_item: false,
            no_cues_for_last_item: false,
            start_and_end_gradually: false,
        }
    }
}

// TODO: add support for overlapper feature
