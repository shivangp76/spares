use crate::{
    AdapterErrorKind, Error, LibraryError,
    parsers::{NoteSettings, Parseable},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use migration::MigrationFunc;
use sqlx::SqlitePool;

pub mod impls;
pub mod migration;

#[async_trait]
pub trait SrsAdapter {
    fn get_adapter_name(&self) -> &'static str;

    async fn migrate(
        &mut self,
        base_url: &str,
        spares_pool: &SqlitePool,
        migration_function: Option<MigrationFunc>,
        initial_migration: bool,
        run: bool,
    ) -> Result<(), Error>;

    // NOTE: notes is NOT `Vec<(NoteSettings, Vec<CardData>)>` since spares just take the `note_data: String`. Other adapters can call `parser.get_cards(note_data)` to get `Vec<CardData>`. Note that the cards are already validated, even though they are not passed as a parameter.
    async fn process_data(
        &mut self,
        notes: Vec<(NoteSettings, Option<String>)>,
        parser: &dyn Parseable,
        run: bool,
        quiet: bool,
        at: DateTime<Utc>,
    ) -> Result<(), Error>;
}

pub fn get_adapter_from_string(adapter_str: &str) -> Result<Box<dyn SrsAdapter>, Error> {
    let all_adapters: Vec<fn() -> Box<dyn SrsAdapter>> = get_all_adapters();
    let matching_adapters: Vec<fn() -> Box<dyn SrsAdapter>> = all_adapters
        .into_iter()
        .filter(|p| adapter_str == p().get_adapter_name())
        .collect();
    if matching_adapters.is_empty() {
        return Err(Error::Library(LibraryError::Adapter(
            AdapterErrorKind::NotFound(adapter_str.to_string()),
        )));
    }
    // Not possible. See `test_adapters_validation`
    // if matching_adapters.len() > 1 {
    // }
    Ok(matching_adapters[0]())
}

pub fn get_all_adapters() -> Vec<fn() -> Box<dyn SrsAdapter>> {
    // NOTE: Add adapter here
    let all_adapters: Vec<fn() -> Box<dyn SrsAdapter>> =
        vec![|| Box::new(impls::anki::AnkiAdapter::new()), || {
            Box::new(impls::spares::SparesAdapter::new(
                impls::spares::SparesRequestProcessor::Server,
            ))
        }];
    all_adapters
}

#[cfg(test)]
mod tests {
    use super::*;
    use itertools::Itertools;

    #[test]
    fn test_adapters_validation() {
        let all_adapters = get_all_adapters();
        assert!(!all_adapters.is_empty());
        let mut all_adapter_names = Vec::new();
        for adapter_fn in all_adapters {
            let adapter = adapter_fn();
            all_adapter_names.push(adapter.get_adapter_name());
            // assert!(validate_adapter(adapter.as_ref()).is_none());
        }
        assert_eq!(
            all_adapter_names.len(),
            all_adapter_names.iter().unique().count()
        );
    }
}
