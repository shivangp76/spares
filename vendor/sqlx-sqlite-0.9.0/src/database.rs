pub(crate) use sqlx_core::database::Database;
pub(crate) use sqlx_core::database::HasStatementCache;

use crate::SqliteArguments;
use crate::SqliteColumn;
use crate::SqliteConnection;
use crate::SqliteQueryResult;
use crate::SqliteRow;
use crate::SqliteStatement;
use crate::SqliteTransactionManager;
use crate::SqliteTypeInfo;
use crate::SqliteValue;
use crate::SqliteValueRef;
use crate::arguments::SqliteArgumentsBuffer;

/// Sqlite database driver.
#[derive(Debug)]
pub struct Sqlite;

impl Database for Sqlite {
    type Connection = SqliteConnection;

    type TransactionManager = SqliteTransactionManager;

    type Row = SqliteRow;

    type QueryResult = SqliteQueryResult;

    type Column = SqliteColumn;

    type TypeInfo = SqliteTypeInfo;

    type Value = SqliteValue;
    type ValueRef<'r> = SqliteValueRef<'r>;

    type Arguments = SqliteArguments;
    type ArgumentBuffer = SqliteArgumentsBuffer;

    type Statement = SqliteStatement;

    const NAME: &'static str = "SQLite";

    const URL_SCHEMES: &'static [&'static str] = &["sqlite"];
}

impl HasStatementCache for Sqlite {}
