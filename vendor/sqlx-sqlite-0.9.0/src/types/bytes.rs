use std::borrow::Cow;
use std::rc::Rc;
use std::sync::Arc;

use crate::Sqlite;
use crate::SqliteArgumentValue;
use crate::SqliteTypeInfo;
use crate::SqliteValueRef;
use crate::arguments::SqliteArgumentsBuffer;
use crate::decode::Decode;
use crate::encode::Encode;
use crate::encode::IsNull;
use crate::error::BoxDynError;
use crate::type_info::DataType;
use crate::types::Type;

impl Type<Sqlite> for [u8] {
    fn type_info() -> SqliteTypeInfo {
        SqliteTypeInfo(DataType::Blob)
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        matches!(ty.0, DataType::Blob | DataType::Text)
    }
}

impl Encode<'_, Sqlite> for &'_ [u8] {
    fn encode_by_ref(&self, args: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        args.push(SqliteArgumentValue::Blob(Arc::new(self.to_vec())));

        Ok(IsNull::No)
    }
}

impl<'r> Decode<'r, Sqlite> for &'r [u8] {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(value.blob_borrowed())
    }
}

impl Encode<'_, Sqlite> for Box<[u8]> {
    fn encode(self, args: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        args.push(SqliteArgumentValue::Blob(Arc::new(self.into_vec())));

        Ok(IsNull::No)
    }

    fn encode_by_ref(&self, args: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        args.push(SqliteArgumentValue::Blob(Arc::new(self.clone().into_vec())));

        Ok(IsNull::No)
    }
}

impl Type<Sqlite> for Vec<u8> {
    fn type_info() -> SqliteTypeInfo {
        <&[u8] as Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <&[u8] as Type<Sqlite>>::compatible(ty)
    }
}

impl Encode<'_, Sqlite> for Vec<u8> {
    fn encode(self, args: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        args.push(SqliteArgumentValue::Blob(Arc::new(self)));

        Ok(IsNull::No)
    }

    fn encode_by_ref(&self, args: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        args.push(SqliteArgumentValue::Blob(Arc::new(self.clone())));

        Ok(IsNull::No)
    }
}

impl<'r> Decode<'r, Sqlite> for Vec<u8> {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(value.blob_owned())
    }
}

impl Encode<'_, Sqlite> for Cow<'_, [u8]> {
    fn encode(self, args: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        args.push(SqliteArgumentValue::Blob(Arc::new(self.into())));

        Ok(IsNull::No)
    }

    fn encode_by_ref(&self, args: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        args.push(SqliteArgumentValue::Blob(Arc::new(self.to_vec())));

        Ok(IsNull::No)
    }
}

impl Encode<'_, Sqlite> for Arc<[u8]> {
    fn encode_by_ref(&self, args: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        <Vec<u8> as Encode<'_, Sqlite>>::encode(self.to_vec(), args)
    }
}

impl Encode<'_, Sqlite> for Rc<[u8]> {
    fn encode_by_ref(&self, args: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        <Vec<u8> as Encode<'_, Sqlite>>::encode(self.to_vec(), args)
    }
}
