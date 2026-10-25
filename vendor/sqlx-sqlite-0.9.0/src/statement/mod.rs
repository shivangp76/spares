use std::sync::Arc;

use sqlx_core::Either;
use sqlx_core::HashMap;
use sqlx_core::sql_str::SqlStr;
pub(crate) use sqlx_core::statement::*;

use crate::Sqlite;
use crate::SqliteArguments;
use crate::SqliteColumn;
use crate::SqliteTypeInfo;
use crate::column::ColumnIndex;
use crate::error::Error;
use crate::ext::ustr::UStr;

mod handle;

#[cfg(feature = "unlock-notify")]
pub(super) mod unlock_notify;
mod r#virtual;

pub(crate) use handle::StatementHandle;
pub(crate) use r#virtual::VirtualStatement;

#[derive(Debug, Clone)]
#[allow(clippy::rc_buffer)]
pub struct SqliteStatement {
    pub(crate) sql: SqlStr,
    pub(crate) parameters: usize,
    pub(crate) columns: Arc<Vec<SqliteColumn>>,
    pub(crate) column_names: Arc<HashMap<UStr, usize>>,
}

impl Statement for SqliteStatement {
    type Database = Sqlite;

    fn into_sql(self) -> SqlStr {
        self.sql
    }

    fn sql(&self) -> &SqlStr {
        &self.sql
    }

    fn parameters(&self) -> Option<Either<&[SqliteTypeInfo], usize>> {
        Some(Either::Right(self.parameters))
    }

    fn columns(&self) -> &[SqliteColumn] {
        &self.columns
    }

    impl_statement_query!(SqliteArguments);
}

impl ColumnIndex<SqliteStatement> for &'_ str {
    fn index(&self, statement: &SqliteStatement) -> Result<usize, Error> {
        statement
            .column_names
            .get(*self)
            .ok_or_else(|| Error::ColumnNotFound((*self).into()))
            .copied()
    }
}
