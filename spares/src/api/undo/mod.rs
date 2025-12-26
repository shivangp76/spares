//! # Undo Functionality
//!
//! ## Outline
//! - To undo an event, you append a new event to the event log that reverse the previous event. For example, to undo `AddNote`, you append a `DeleteNote` event.
//!
//! ## Problems and Solutions
//! - Future: Syncing data between devices
//!   - Last write wins. This is why there is a `timestamp` field. Events are merged and then replayed in chronological order. If there is a conflict, pick random one for now. This can be worked out later. (Pretty unlikely there is a conflict at the exact same timestamp.)
//!   - Future: Add `device_id` field to `Event`
//! - Branching undo logs
//!   - Events can be undone by their id, so the user can submit the exact action they want to undo.
//!   - If the user does `create parser -> add note to parser -> UNDO: create parser`, then throw error saying cannot delete parser since notes depend on it.
//! - Importing a bunch of notes at once. This should all be undone at once.
//!   - `group_id` field, so all those actions will be undone at once
//! - Schema for payload changes
//!   - Handled by `version` field

use crate::model::{Event, EventType};
