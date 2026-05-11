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

const MAX_ROWS_IN_QUERY: usize = 200;

pub use card::create_card_tags;
pub use card::delete_card_tags;
pub use card::forget_card;
pub use card::get_card;
pub use card::get_cards;
pub use card::get_leeches;
pub use card::update_cards;
use sqlx::SqlitePool;

use crate::Error;

pub(crate) fn placeholders(rows: usize) -> String {
    std::iter::repeat_n("?", rows)
        .collect::<Vec<&str>>()
        .join(", ")
}

pub(crate) fn placeholders_2d(rows: usize, cols: usize) -> String {
    let tuple = std::iter::repeat_n("?", cols)
        .collect::<Vec<_>>()
        .join(", ");
    std::iter::repeat_n(format!("({})", tuple), rows)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Chunks the input rows into batches of `MAX_ROWS_IN_QUERY` to avoid "too many SQL variables" errors.
async fn fetch_batched_query<'a, T, R, F, Fut>(
    db: &'a SqlitePool,
    rows: &'a [T],
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
    let chunks = rows.chunks(MAX_ROWS_IN_QUERY).collect::<Vec<_>>();
    let mut all_results = Vec::new();
    for chunk in chunks {
        let chunk_results = query_fn(db, chunk).await?;
        all_results.extend(chunk_results);
    }
    Ok(all_results)
}

/// Chunks the input rows into batches of `MAX_ROWS_IN_QUERY` to avoid "too many SQL variables" errors.
async fn execute_batched_query<'a, T, F, Fut>(
    db: &'a SqlitePool,
    rows: &'a [T],
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
    let chunks = rows.chunks(MAX_ROWS_IN_QUERY).collect::<Vec<_>>();
    for chunk in chunks {
        query_fn(db, chunk).await?;
    }
    Ok(())
}
