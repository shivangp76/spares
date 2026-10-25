use serde::Deserialize;
use serde::Serialize;

use crate::Sqlite;
use crate::SqliteTypeInfo;
use crate::SqliteValueRef;
use crate::arguments::SqliteArgumentsBuffer;
use crate::decode::Decode;
use crate::encode::Encode;
use crate::encode::IsNull;
use crate::error::BoxDynError;
use crate::type_info::DataType;
use crate::types::Json;
use crate::types::Type;

impl<T> Type<Sqlite> for Json<T> {
    fn type_info() -> SqliteTypeInfo {
        SqliteTypeInfo(DataType::Text)
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <&str as Type<Sqlite>>::compatible(ty)
    }
}

impl<T> Encode<'_, Sqlite> for Json<T>
where
    T: Serialize,
{
    fn encode_by_ref(&self, buf: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        Encode::<Sqlite>::encode(self.encode_to_string()?, buf)
    }
}

impl<'r, T> Decode<'r, Sqlite> for Json<T>
where
    T: 'r + Deserialize<'r>,
{
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        // Saves a pass over the data by making `serde_json` check UTF-8.
        Self::decode_from_bytes(Decode::<Sqlite>::decode(value)?)
    }
}
