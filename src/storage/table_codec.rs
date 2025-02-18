use crate::catalog::{ColumnCatalog, TableMeta};
use crate::errors::DatabaseError;
use crate::types::index::{Index, IndexId, IndexMeta, IndexType};
use crate::types::tuple::{Schema, Tuple, TupleId};
use crate::types::value::DataValue;
use crate::types::LogicalType;
use bytes::Bytes;
use integer_encoding::FixedInt;
use lazy_static::lazy_static;
use std::sync::Arc;

const BOUND_MIN_TAG: u8 = 0;
const BOUND_MAX_TAG: u8 = 1;

lazy_static! {
    static ref ROOT_BYTES: Vec<u8> = b"Root".to_vec();
}

#[derive(Clone)]
pub struct TableCodec {}

#[derive(Copy, Clone)]
enum CodecType {
    Column,
    IndexMeta,
    Index,
    Statistics,
    Tuple,
    Root,
}

impl TableCodec {
    /// TableName + Type
    ///
    /// Tips: Root full key = key_prefix
    fn key_prefix(ty: CodecType, table_name: &str) -> Vec<u8> {
        let mut table_bytes = table_name.to_string().into_bytes();

        match ty {
            CodecType::Column => {
                table_bytes.push(b'0');
            }
            CodecType::IndexMeta => {
                table_bytes.push(b'1');
            }
            CodecType::Index => {
                table_bytes.push(b'3');
            }
            CodecType::Statistics => {
                table_bytes.push(b'4');
            }
            CodecType::Tuple => {
                table_bytes.push(b'8');
            }
            CodecType::Root => {
                let mut bytes = ROOT_BYTES.clone();
                bytes.push(BOUND_MIN_TAG);
                bytes.append(&mut table_bytes);

                return bytes;
            }
        }

        table_bytes
    }

    pub fn tuple_bound(table_name: &str) -> (Vec<u8>, Vec<u8>) {
        let op = |bound_id| {
            let mut key_prefix = Self::key_prefix(CodecType::Tuple, table_name);

            key_prefix.push(bound_id);
            key_prefix
        };

        (op(BOUND_MIN_TAG), op(BOUND_MAX_TAG))
    }

    pub fn index_meta_bound(table_name: &str) -> (Vec<u8>, Vec<u8>) {
        let op = |bound_id| {
            let mut key_prefix = Self::key_prefix(CodecType::IndexMeta, table_name);

            key_prefix.push(bound_id);
            key_prefix
        };

        (op(BOUND_MIN_TAG), op(BOUND_MAX_TAG))
    }

    pub fn index_bound(table_name: &str, index_id: &IndexId) -> (Vec<u8>, Vec<u8>) {
        let op = |bound_id| {
            let mut key_prefix = Self::key_prefix(CodecType::Index, table_name);

            key_prefix.push(BOUND_MIN_TAG);
            key_prefix.append(&mut index_id.to_be_bytes().to_vec());
            key_prefix.push(bound_id);
            key_prefix
        };

        (op(BOUND_MIN_TAG), op(BOUND_MAX_TAG))
    }

    pub fn all_index_bound(table_name: &str) -> (Vec<u8>, Vec<u8>) {
        let op = |bound_id| {
            let mut key_prefix = Self::key_prefix(CodecType::Index, table_name);

            key_prefix.push(bound_id);
            key_prefix
        };

        (op(BOUND_MIN_TAG), op(BOUND_MAX_TAG))
    }

    pub fn root_table_bound() -> (Vec<u8>, Vec<u8>) {
        let op = |bound_id| {
            let mut key_prefix = ROOT_BYTES.clone();

            key_prefix.push(bound_id);
            key_prefix
        };

        (op(BOUND_MIN_TAG), op(BOUND_MAX_TAG))
    }

    pub fn table_bound(table_name: &str) -> (Vec<u8>, Vec<u8>) {
        let mut column_prefix = Self::key_prefix(CodecType::Column, table_name);
        column_prefix.push(BOUND_MIN_TAG);

        let mut index_prefix = Self::key_prefix(CodecType::IndexMeta, table_name);
        index_prefix.push(BOUND_MAX_TAG);

        (column_prefix, index_prefix)
    }

    pub fn columns_bound(table_name: &str) -> (Vec<u8>, Vec<u8>) {
        let op = |bound_id| {
            let mut key_prefix = Self::key_prefix(CodecType::Column, table_name);

            key_prefix.push(bound_id);
            key_prefix
        };

        (op(BOUND_MIN_TAG), op(BOUND_MAX_TAG))
    }

    /// Key: {TableName}{TUPLE_TAG}{BOUND_MIN_TAG}{RowID}(Sorted)
    /// Value: Tuple
    pub fn encode_tuple(
        table_name: &str,
        tuple: &Tuple,
        types: &[LogicalType],
    ) -> Result<(Bytes, Bytes), DatabaseError> {
        let tuple_id = tuple.id.clone().ok_or(DatabaseError::PrimaryKeyNotFound)?;
        let key = Self::encode_tuple_key(table_name, &tuple_id)?;

        Ok((Bytes::from(key), Bytes::from(tuple.serialize_to(types)?)))
    }

    pub fn encode_tuple_key(
        table_name: &str,
        tuple_id: &TupleId,
    ) -> Result<Vec<u8>, DatabaseError> {
        let mut key_prefix = Self::key_prefix(CodecType::Tuple, table_name);
        key_prefix.push(BOUND_MIN_TAG);

        if !matches!(
            tuple_id.logical_type(),
            LogicalType::Tinyint
                | LogicalType::Smallint
                | LogicalType::Integer
                | LogicalType::Bigint
                | LogicalType::UTinyint
                | LogicalType::USmallint
                | LogicalType::UInteger
                | LogicalType::UBigint
                | LogicalType::Varchar(_)
        ) {
            return Err(DatabaseError::InvalidType);
        }
        tuple_id.memcomparable_encode(&mut key_prefix)?;

        Ok(key_prefix)
    }

    pub fn decode_tuple(
        table_types: &[LogicalType],
        projections: &[usize],
        schema: &Schema,
        bytes: &[u8],
    ) -> Tuple {
        Tuple::deserialize_from(table_types, projections, schema, bytes)
    }

    /// Key: {TableName}{INDEX_META_TAG}{BOUND_MIN_TAG}{IndexID}
    /// Value: IndexMeta
    pub fn encode_index_meta(
        table_name: &str,
        index_meta: &IndexMeta,
    ) -> Result<(Bytes, Bytes), DatabaseError> {
        let mut key_prefix = Self::key_prefix(CodecType::IndexMeta, table_name);
        key_prefix.push(BOUND_MIN_TAG);
        key_prefix.append(&mut index_meta.id.to_be_bytes().to_vec());

        Ok((
            Bytes::from(key_prefix),
            Bytes::from(bincode::serialize(&index_meta)?),
        ))
    }

    pub fn decode_index_meta(bytes: &[u8]) -> Result<IndexMeta, DatabaseError> {
        Ok(bincode::deserialize(bytes)?)
    }

    /// NonUnique Index:
    /// Key: {TableName}{INDEX_TAG}{BOUND_MIN_TAG}{IndexID}{BOUND_MIN_TAG}{DataValue1}{DataValue2} .. {TupleId}
    /// Value: TupleID
    ///
    /// Unique Index:
    /// Key: {TableName}{INDEX_TAG}{BOUND_MIN_TAG}{IndexID}{BOUND_MIN_TAG}{DataValue}
    /// Value: TupleID
    ///
    /// Tips: The unique index has only one ColumnID and one corresponding DataValue,
    /// so it can be positioned directly.
    pub fn encode_index(
        name: &str,
        index: &Index,
        tuple_id: &TupleId,
    ) -> Result<(Bytes, Bytes), DatabaseError> {
        let key = TableCodec::encode_index_key(name, index, Some(tuple_id))?;
        let mut bytes = Vec::new();
        tuple_id.to_raw(&mut bytes)?;

        Ok((Bytes::from(key), Bytes::from(bytes)))
    }

    fn _encode_index_key(name: &str, index: &Index) -> Result<Vec<u8>, DatabaseError> {
        let mut key_prefix = Self::key_prefix(CodecType::Index, name);
        key_prefix.push(BOUND_MIN_TAG);
        key_prefix.extend_from_slice(&index.id.to_be_bytes());
        key_prefix.push(BOUND_MIN_TAG);

        for col_v in index.column_values {
            col_v.memcomparable_encode(&mut key_prefix)?;
            key_prefix.push(BOUND_MIN_TAG);
        }
        Ok(key_prefix)
    }

    pub fn encode_index_bound_key(
        name: &str,
        index: &Index,
        is_upper: bool,
    ) -> Result<Vec<u8>, DatabaseError> {
        let mut key_prefix = Self::_encode_index_key(name, index)?;

        if is_upper {
            if let Some(last) = key_prefix.last_mut() {
                *last = BOUND_MAX_TAG
            }
        }
        Ok(key_prefix)
    }

    pub fn encode_index_key(
        name: &str,
        index: &Index,
        tuple_id: Option<&TupleId>,
    ) -> Result<Vec<u8>, DatabaseError> {
        let mut key_prefix = Self::_encode_index_key(name, index)?;

        if let Some(tuple_id) = tuple_id {
            if matches!(index.ty, IndexType::Normal | IndexType::Composite) {
                tuple_id.to_raw(&mut key_prefix)?;
            }
        }
        Ok(key_prefix)
    }

    pub fn decode_index(bytes: &[u8], primary_key_ty: &LogicalType) -> TupleId {
        Arc::new(DataValue::from_raw(bytes, primary_key_ty))
    }

    /// Key: {TableName}{COLUMN_TAG}{BOUND_MIN_TAG}{ColumnId}
    /// Value: ColumnCatalog
    ///
    /// Tips: the `0` for bound range
    pub fn encode_column(
        table_name: &str,
        col: &ColumnCatalog,
    ) -> Result<(Bytes, Bytes), DatabaseError> {
        let mut key_prefix = Self::key_prefix(CodecType::Column, table_name);

        key_prefix.push(BOUND_MIN_TAG);
        key_prefix.append(&mut col.id().unwrap().to_be_bytes().to_vec());

        Ok((
            Bytes::from(key_prefix),
            Bytes::from(bincode::serialize(col)?),
        ))
    }

    pub fn decode_column(bytes: &[u8]) -> Result<ColumnCatalog, DatabaseError> {
        Ok(bincode::deserialize::<ColumnCatalog>(bytes)?)
    }

    /// Key: {TableName}{STATISTICS_TAG}{BOUND_MIN_TAG}{INDEX_ID}
    /// Value: StatisticsMeta Path
    pub fn encode_statistics_path(
        table_name: &str,
        index_id: IndexId,
        path: String,
    ) -> (Bytes, Bytes) {
        let key = Self::encode_statistics_path_key(table_name, index_id);

        (Bytes::from(key), Bytes::from(path))
    }

    pub fn encode_statistics_path_key(table_name: &str, index_id: IndexId) -> Vec<u8> {
        let mut key_prefix = Self::key_prefix(CodecType::Statistics, table_name);

        key_prefix.push(BOUND_MIN_TAG);
        key_prefix.extend_from_slice(index_id.encode_fixed_light());
        key_prefix
    }

    pub fn decode_statistics_path(bytes: &[u8]) -> Result<String, DatabaseError> {
        Ok(String::from_utf8(bytes.to_vec())?)
    }

    /// Key: Root{BOUND_MIN_TAG}{TableName}
    /// Value: TableMeta
    pub fn encode_root_table(meta: &TableMeta) -> Result<(Bytes, Bytes), DatabaseError> {
        let key = Self::encode_root_table_key(&meta.table_name);

        Ok((Bytes::from(key), Bytes::from(bincode::serialize(meta)?)))
    }

    pub fn encode_root_table_key(table_name: &str) -> Vec<u8> {
        Self::key_prefix(CodecType::Root, table_name)
    }

    pub fn decode_root_table(bytes: &[u8]) -> Result<TableMeta, DatabaseError> {
        Ok(bincode::deserialize(bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use crate::catalog::{ColumnCatalog, ColumnDesc, TableCatalog, TableMeta};
    use crate::errors::DatabaseError;
    use crate::storage::table_codec::TableCodec;
    use crate::types::index::{Index, IndexMeta, IndexType};
    use crate::types::tuple::Tuple;
    use crate::types::value::DataValue;
    use crate::types::LogicalType;
    use bytes::Bytes;
    use itertools::Itertools;
    use rust_decimal::Decimal;
    use std::collections::BTreeSet;
    use std::ops::Bound;
    use std::slice;
    use std::sync::Arc;

    fn build_table_codec() -> TableCatalog {
        let columns = vec![
            ColumnCatalog::new(
                "c1".into(),
                false,
                ColumnDesc::new(LogicalType::Integer, true, false, None),
            ),
            ColumnCatalog::new(
                "c2".into(),
                false,
                ColumnDesc::new(LogicalType::Decimal(None, None), false, false, None),
            ),
        ];
        TableCatalog::new(Arc::new("t1".to_string()), columns).unwrap()
    }

    #[test]
    fn test_table_codec_tuple() -> Result<(), DatabaseError> {
        let table_catalog = build_table_codec();

        let tuple = Tuple {
            id: Some(Arc::new(DataValue::Int32(Some(0)))),
            values: vec![
                Arc::new(DataValue::Int32(Some(0))),
                Arc::new(DataValue::Decimal(Some(Decimal::new(1, 0)))),
            ],
        };
        let (_, bytes) = TableCodec::encode_tuple(
            &table_catalog.name,
            &tuple,
            &[LogicalType::Integer, LogicalType::Decimal(None, None)],
        )?;
        let schema = table_catalog.schema_ref();

        assert_eq!(
            TableCodec::decode_tuple(&table_catalog.types(), &[0, 1], schema, &bytes),
            tuple
        );

        Ok(())
    }

    #[test]
    fn test_root_catalog() {
        let table_catalog = build_table_codec();
        let (_, bytes) = TableCodec::encode_root_table(&TableMeta {
            table_name: table_catalog.name.clone(),
        })
        .unwrap();

        let table_meta = TableCodec::decode_root_table(&bytes).unwrap();

        assert_eq!(table_meta.table_name.as_str(), table_catalog.name.as_str());
    }

    #[test]
    fn test_table_codec_statistics_meta_path() {
        let path = String::from("./lol");
        let (_, bytes) = TableCodec::encode_statistics_path("t1", 0, path.clone());
        let decode_path = TableCodec::decode_statistics_path(&bytes).unwrap();

        assert_eq!(path, decode_path);
    }

    #[test]
    fn test_table_codec_index_meta() -> Result<(), DatabaseError> {
        let index_meta = IndexMeta {
            id: 0,
            column_ids: vec![0],
            table_name: Arc::new("T1".to_string()),
            pk_ty: LogicalType::Integer,
            name: "index_1".to_string(),
            ty: IndexType::PrimaryKey,
        };
        let (_, bytes) = TableCodec::encode_index_meta(&"T1".to_string(), &index_meta)?;

        assert_eq!(TableCodec::decode_index_meta(&bytes)?, index_meta);

        Ok(())
    }

    #[test]
    fn test_table_codec_index() -> Result<(), DatabaseError> {
        let table_catalog = build_table_codec();
        let value = Arc::new(DataValue::Int32(Some(0)));
        let index = Index::new(0, slice::from_ref(&value), IndexType::PrimaryKey);
        let tuple_id = Arc::new(DataValue::Int32(Some(0)));
        let (_, bytes) = TableCodec::encode_index(&table_catalog.name, &index, &tuple_id)?;

        assert_eq!(
            TableCodec::decode_index(&bytes, &tuple_id.logical_type()),
            tuple_id
        );

        Ok(())
    }

    #[test]
    fn test_table_codec_column() {
        let table_catalog = build_table_codec();
        let col = table_catalog.columns().next().cloned().unwrap();

        let (_, bytes) = TableCodec::encode_column(&table_catalog.name, &col).unwrap();
        let decode_col = TableCodec::decode_column(&bytes).unwrap();

        assert_eq!(&decode_col, col.as_ref());
    }

    #[test]
    fn test_table_codec_column_bound() {
        let mut set = BTreeSet::new();
        let op = |col_id: usize, table_name: &str| {
            let mut col = ColumnCatalog::new(
                "".to_string(),
                false,
                ColumnDesc {
                    column_datatype: LogicalType::Invalid,
                    is_primary: false,
                    is_unique: false,
                    default: None,
                },
            );

            col.summary.id = Some(col_id as u32);

            let (key, _) = TableCodec::encode_column(&table_name.to_string(), &col).unwrap();
            key
        };

        set.insert(op(0, "T0"));
        set.insert(op(1, "T0"));
        set.insert(op(2, "T0"));

        set.insert(op(0, "T1"));
        set.insert(op(1, "T1"));
        set.insert(op(2, "T1"));

        set.insert(op(0, "T2"));
        set.insert(op(0, "T2"));
        set.insert(op(0, "T2"));

        let (min, max) = TableCodec::columns_bound(&Arc::new("T1".to_string()));

        let vec = set
            .range::<Bytes, (Bound<&Bytes>, Bound<&Bytes>)>((
                Bound::Included(&Bytes::from(min)),
                Bound::Included(&Bytes::from(max)),
            ))
            .collect_vec();

        assert_eq!(vec.len(), 3);

        assert_eq!(vec[0], &op(0, "T1"));
        assert_eq!(vec[1], &op(1, "T1"));
        assert_eq!(vec[2], &op(2, "T1"));
    }

    #[test]
    fn test_table_codec_index_meta_bound() {
        let mut set = BTreeSet::new();
        let op = |index_id: usize, table_name: &str| {
            let index_meta = IndexMeta {
                id: index_id as u32,
                column_ids: vec![],
                table_name: Arc::new(table_name.to_string()),
                pk_ty: LogicalType::Integer,
                name: "".to_string(),
                ty: IndexType::PrimaryKey,
            };

            let (key, _) =
                TableCodec::encode_index_meta(&table_name.to_string(), &index_meta).unwrap();
            key
        };

        set.insert(op(0, "T0"));
        set.insert(op(1, "T0"));
        set.insert(op(2, "T0"));

        set.insert(op(0, "T1"));
        set.insert(op(1, "T1"));
        set.insert(op(2, "T1"));

        set.insert(op(0, "T2"));
        set.insert(op(1, "T2"));
        set.insert(op(2, "T2"));

        let (min, max) = TableCodec::index_meta_bound(&"T1".to_string());

        let vec = set
            .range::<Bytes, (Bound<&Bytes>, Bound<&Bytes>)>((
                Bound::Included(&Bytes::from(min)),
                Bound::Included(&Bytes::from(max)),
            ))
            .collect_vec();

        assert_eq!(vec.len(), 3);

        assert_eq!(vec[0], &op(0, "T1"));
        assert_eq!(vec[1], &op(1, "T1"));
        assert_eq!(vec[2], &op(2, "T1"));
    }

    #[test]
    fn test_table_codec_index_bound() {
        let mut set = BTreeSet::new();
        let column = ColumnCatalog::new(
            "".to_string(),
            false,
            ColumnDesc::new(LogicalType::Boolean, false, false, None),
        );
        let table_catalog = TableCatalog::new(Arc::new("T0".to_string()), vec![column]).unwrap();

        let op = |value: DataValue, index_id: usize, table_name: &String| {
            let value = Arc::new(value);
            let index = Index::new(
                index_id as u32,
                slice::from_ref(&value),
                IndexType::PrimaryKey,
            );

            TableCodec::encode_index_key(table_name, &index, None).unwrap()
        };

        set.insert(op(DataValue::Int32(Some(0)), 0, &table_catalog.name));
        set.insert(op(DataValue::Int32(Some(1)), 0, &table_catalog.name));
        set.insert(op(DataValue::Int32(Some(2)), 0, &table_catalog.name));

        set.insert(op(DataValue::Int32(Some(0)), 1, &table_catalog.name));
        set.insert(op(DataValue::Int32(Some(1)), 1, &table_catalog.name));
        set.insert(op(DataValue::Int32(Some(2)), 1, &table_catalog.name));

        set.insert(op(DataValue::Int32(Some(0)), 2, &table_catalog.name));
        set.insert(op(DataValue::Int32(Some(1)), 2, &table_catalog.name));
        set.insert(op(DataValue::Int32(Some(2)), 2, &table_catalog.name));

        println!("{:#?}", set);

        let (min, max) = TableCodec::index_bound(&table_catalog.name, &1);

        println!("{:?}", min);
        println!("{:?}", max);

        let vec = set
            .range::<Vec<u8>, (Bound<&Vec<u8>>, Bound<&Vec<u8>>)>((
                Bound::Included(&min),
                Bound::Included(&max),
            ))
            .collect_vec();

        assert_eq!(vec.len(), 3);

        assert_eq!(
            vec[0],
            &op(DataValue::Int32(Some(0)), 1, &table_catalog.name)
        );
        assert_eq!(
            vec[1],
            &op(DataValue::Int32(Some(1)), 1, &table_catalog.name)
        );
        assert_eq!(
            vec[2],
            &op(DataValue::Int32(Some(2)), 1, &table_catalog.name)
        );
    }

    #[test]
    fn test_table_codec_index_all_bound() {
        let mut set = BTreeSet::new();
        let op = |value: DataValue, index_id: usize, table_name: &str| {
            let value = Arc::new(value);
            let index = Index::new(
                index_id as u32,
                slice::from_ref(&value),
                IndexType::PrimaryKey,
            );

            TableCodec::encode_index_key(&table_name.to_string(), &index, None).unwrap()
        };

        set.insert(op(DataValue::Int32(Some(0)), 0, "T0"));
        set.insert(op(DataValue::Int32(Some(1)), 0, "T0"));
        set.insert(op(DataValue::Int32(Some(2)), 0, "T0"));

        set.insert(op(DataValue::Int32(Some(0)), 0, "T1"));
        set.insert(op(DataValue::Int32(Some(1)), 0, "T1"));
        set.insert(op(DataValue::Int32(Some(2)), 0, "T1"));

        set.insert(op(DataValue::Int32(Some(0)), 0, "T2"));
        set.insert(op(DataValue::Int32(Some(1)), 0, "T2"));
        set.insert(op(DataValue::Int32(Some(2)), 0, "T2"));

        let (min, max) = TableCodec::all_index_bound(&"T1".to_string());

        let vec = set
            .range::<Vec<u8>, (Bound<&Vec<u8>>, Bound<&Vec<u8>>)>((
                Bound::Included(&min),
                Bound::Included(&max),
            ))
            .collect_vec();

        assert_eq!(vec.len(), 3);

        assert_eq!(vec[0], &op(DataValue::Int32(Some(0)), 0, "T1"));
        assert_eq!(vec[1], &op(DataValue::Int32(Some(1)), 0, "T1"));
        assert_eq!(vec[2], &op(DataValue::Int32(Some(2)), 0, "T1"));
    }

    #[test]
    fn test_table_codec_tuple_bound() {
        let mut set = BTreeSet::new();
        let op = |tuple_id: DataValue, table_name: &str| {
            TableCodec::encode_tuple_key(&table_name.to_string(), &Arc::new(tuple_id)).unwrap()
        };

        set.insert(op(DataValue::Int32(Some(0)), "T0"));
        set.insert(op(DataValue::Int32(Some(1)), "T0"));
        set.insert(op(DataValue::Int32(Some(2)), "T0"));

        set.insert(op(DataValue::Int32(Some(0)), "T1"));
        set.insert(op(DataValue::Int32(Some(1)), "T1"));
        set.insert(op(DataValue::Int32(Some(2)), "T1"));

        set.insert(op(DataValue::Int32(Some(0)), "T2"));
        set.insert(op(DataValue::Int32(Some(1)), "T2"));
        set.insert(op(DataValue::Int32(Some(2)), "T2"));

        let (min, max) = TableCodec::tuple_bound(&"T1".to_string());

        let vec = set
            .range::<Vec<u8>, (Bound<&Vec<u8>>, Bound<&Vec<u8>>)>((
                Bound::Included(&min),
                Bound::Included(&max),
            ))
            .collect_vec();

        assert_eq!(vec.len(), 3);

        assert_eq!(vec[0], &op(DataValue::Int32(Some(0)), "T1"));
        assert_eq!(vec[1], &op(DataValue::Int32(Some(1)), "T1"));
        assert_eq!(vec[2], &op(DataValue::Int32(Some(2)), "T1"));
    }

    #[test]
    fn test_root_codec_name_bound() {
        let mut set = BTreeSet::new();
        let op = |table_name: &str| TableCodec::encode_root_table_key(&table_name.to_string());

        set.insert(b"A".to_vec());

        set.insert(op("T0"));
        set.insert(op("T1"));
        set.insert(op("T2"));

        set.insert(b"Z".to_vec());

        let (min, max) = TableCodec::root_table_bound();

        let vec = set
            .range::<Vec<u8>, (Bound<&Vec<u8>>, Bound<&Vec<u8>>)>((
                Bound::Included(&min),
                Bound::Included(&max),
            ))
            .collect_vec();

        assert_eq!(vec[0], &op("T0"));
        assert_eq!(vec[1], &op("T1"));
        assert_eq!(vec[2], &op("T2"));
    }
}
