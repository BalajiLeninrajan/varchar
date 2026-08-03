use super::{encode_table_metadata, measure_table_metadata};
use crate::storage::{ForeignKey, TableSchema};
use crate::{DataType, SchemaColumn, Value};

fn column(name: &str, data_type: DataType, nullable: bool, default: Option<Value>) -> SchemaColumn {
    SchemaColumn {
        name: String::from(name),
        data_type,
        nullable,
        default,
    }
}

#[test]
fn exact_measurement_preserves_every_metadata_phase() {
    let schema = TableSchema {
        name: String::from("all_meta"),
        columns: vec![
            column("id", DataType::Integer, false, None),
            column(
                "parent_id",
                DataType::Integer,
                true,
                Some(Value::Integer(i64::MIN)),
            ),
            column(
                "text",
                DataType::Text,
                true,
                Some(Value::Text(String::from("é|%\0"))),
            ),
            column("flag", DataType::Boolean, true, Some(Value::Boolean(true))),
            column("note", DataType::Text, true, Some(Value::Null)),
        ],
        primary_key: Some(0),
        unique_columns: vec![2, 3],
        foreign_keys: vec![ForeignKey {
            column: 1,
            referenced_table: String::from("parent"),
            referenced_column: String::from("id"),
        }],
        checks: Vec::new(),
    };
    let expected = concat!(
        "~S|all_meta|id:I:!|parent_id:I:?|text:T:?|flag:B:?|note:T:?;",
        "~P|all_meta|id;",
        "~F|all_meta|parent_id|parent|id;",
        "~A|all_meta|id|I0;",
        "~D|all_meta|parent_id|I-9223372036854775808;",
        "~D|all_meta|text|Té%00007C%000025%000000;",
        "~D|all_meta|flag|B1;",
        "~D|all_meta|note|N;",
        "~U|all_meta|text;",
        "~U|all_meta|flag;",
    );

    let measured = measure_table_metadata(&schema, Some((0, 0))).expect("metadata measures");
    assert_eq!(measured.encoded_len(), expected.len());
    assert_eq!(
        encode_table_metadata(&schema, Some((0, 0)), measured).expect("metadata encodes"),
        expected
    );
}
