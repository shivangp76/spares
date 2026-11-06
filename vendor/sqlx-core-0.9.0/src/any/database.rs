use crate::any::AnyArgumentBuffer;
use crate::any::AnyArguments;
use crate::any::AnyColumn;
use crate::any::AnyConnection;
use crate::any::AnyQueryResult;
use crate::any::AnyRow;
use crate::any::AnyStatement;
use crate::any::AnyTransactionManager;
use crate::any::AnyTypeInfo;
use crate::any::AnyValue;
use crate::any::AnyValueRef;
use crate::database::Database;
use crate::database::HasStatementCache;

/// Opaque database driver. Capable of being used in place of any SQLx database driver. The actual
/// driver used will be selected at runtime, from the connection url.
#[derive(Debug)]
pub struct Any;

impl Database for Any {
    type Connection = AnyConnection;

    type TransactionManager = AnyTransactionManager;

    type Row = AnyRow;

    type QueryResult = AnyQueryResult;

    type Column = AnyColumn;

    type TypeInfo = AnyTypeInfo;

    type Value = AnyValue;
    type ValueRef<'r> = AnyValueRef<'r>;

    type Arguments = AnyArguments;
    type ArgumentBuffer = AnyArgumentBuffer;

    type Statement = AnyStatement;

    const NAME: &'static str = "Any";

    const URL_SCHEMES: &'static [&'static str] = &[];
}

// This _may_ be true, depending on the selected database
impl HasStatementCache for Any {}
