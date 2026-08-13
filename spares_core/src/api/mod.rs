pub mod card;
pub mod note;
pub mod parser;
pub mod review;
pub mod scheduler;
pub mod statistics;
pub mod tag;
#[cfg(test)]
pub(crate) mod tests;
pub mod undo;

/// `SQLITE_MAX_VARIABLE_NUMBER` for builds linking `SQLite` < 3.32. Newer `SQLite` defaults to
/// 32766; batching against the smaller legacy value keeps statements portable to both.
const SQLITE_MAX_VARIABLE_NUMBER: usize = 999;

/// The maximum number of rows a batched statement may carry when each row binds `columns`
/// parameters, so the statement never exceeds `SQLITE_MAX_VARIABLE_NUMBER` bound variables.
pub(crate) const fn max_rows_for(columns: usize) -> usize {
    SQLITE_MAX_VARIABLE_NUMBER / columns
}

/// Default chunk size for batched statements that bind few parameters per row (1-3).
/// Statements binding more parameters per row use [`max_rows_for`] instead.
pub(crate) const MAX_ROWS_IN_QUERY: usize = 200;

pub use card::create_card_tags;
pub use card::delete_card_tags;
pub use card::forget_card;
pub use card::get_card;
pub use card::get_cards;
pub use card::get_leeches;
pub use card::update_cards;
use sqlx::SqlitePool;

use crate::Error;
use crate::LibraryError;
use crate::SchedulerErrorKind;
use crate::model::SpecialState;

pub(crate) fn placeholders(rows: usize) -> String {
    if rows == 0 {
        return String::new();
    }
    let mut s = String::with_capacity(rows * 2 - 1);
    for i in 0..rows {
        if i > 0 {
            s.push_str(", ");
        }
        s.push('?');
    }
    s
}

pub(crate) fn placeholders_2d(rows: usize, cols: usize) -> String {
    let mut s = String::with_capacity(rows * (cols * 3 + 2));
    for r in 0..rows {
        if r > 0 {
            s.push_str(", ");
        }
        s.push('(');
        for c in 0..cols {
            if c > 0 {
                s.push_str(", ");
            }
            s.push('?');
        }
        s.push(')');
    }
    s
}

/// Fetches rows for `rows`, batching into chunks of at most `chunk_size` so the number of
/// bound parameters per statement stays below `SQLITE_MAX_VARIABLE_NUMBER`.
async fn fetch_batched_query<'a, T, R, F, Fut>(
    db: &'a SqlitePool,
    rows: &'a [T],
    chunk_size: usize,
    query_fn: F,
) -> Result<Vec<R>, Error>
where
    T: Clone,
    F: Fn(&'a SqlitePool, &'a [T]) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<R>, Error>> + 'a,
{
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut all_results = Vec::new();
    for chunk in rows.chunks(chunk_size) {
        let chunk_results = query_fn(db, chunk).await?;
        all_results.extend(chunk_results);
    }
    Ok(all_results)
}

/// Executes `query_fn` for `rows`, batching into chunks of at most `chunk_size`.
async fn execute_batched_query<'a, T, F, Fut>(
    db: &'a SqlitePool,
    rows: &'a [T],
    chunk_size: usize,
    query_fn: F,
) -> Result<(), Error>
where
    T: Clone,
    F: Fn(&'a SqlitePool, &'a [T]) -> Fut,
    Fut: std::future::Future<Output = Result<(), Error>> + 'a,
{
    if rows.is_empty() {
        return Ok(());
    }
    for chunk in rows.chunks(chunk_size) {
        query_fn(db, chunk).await?;
    }
    Ok(())
}

pub(crate) fn validate_bury_target(special_state: Option<SpecialState>) -> Result<(), Error> {
    if let Some(special_state) = special_state {
        match special_state {
            SpecialState::Suspended => {
                return Err(Error::Library(LibraryError::Scheduler(
                    SchedulerErrorKind::Suspended,
                )));
            }
            SpecialState::UserBuried | SpecialState::SchedulerBuried => {
                return Err(Error::Library(LibraryError::Scheduler(
                    SchedulerErrorKind::AlreadyBuried,
                )));
            }
            SpecialState::BuriedUntilLaterToday => {}
        }
    }
    Ok(())
}
