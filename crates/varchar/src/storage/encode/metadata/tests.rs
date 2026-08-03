use super::{
    encode_auto_increment_record, encode_auto_increment_record_prevalidated, encode_table_metadata,
    encoded_auto_increment_record_len, encoded_auto_increment_record_len_prevalidated,
    measure_table_metadata,
};
use crate::expression::{CheckPredicate, CheckProgram, CheckProgramNode, LikeAtom};
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

fn predicate(predicate: CheckPredicate) -> CheckProgram {
    CheckProgram::new(vec![CheckProgramNode::Predicate(predicate)])
}

#[test]
fn exact_measurement_preserves_every_metadata_phase_and_check_value_shape() {
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
        checks: vec![
            CheckProgram::new(vec![
                CheckProgramNode::And { children: 2 },
                CheckProgramNode::Predicate(CheckPredicate::Equal {
                    column: 0,
                    value: Value::Integer(i64::MIN),
                }),
                CheckProgramNode::Or { children: 2 },
                CheckProgramNode::Predicate(CheckPredicate::NotEqual {
                    column: 2,
                    value: Value::Text(String::from("x|")),
                }),
                CheckProgramNode::Predicate(CheckPredicate::LessThan {
                    column: 3,
                    value: Value::Boolean(true),
                }),
            ]),
            predicate(CheckPredicate::LessThanOrEqual {
                column: 0,
                value: Value::Integer(0),
            }),
            predicate(CheckPredicate::GreaterThan {
                column: 0,
                value: Value::Integer(-1),
            }),
            predicate(CheckPredicate::GreaterThanOrEqual {
                column: 0,
                value: Value::Integer(i64::MAX),
            }),
            predicate(CheckPredicate::Like {
                column: 2,
                atoms: vec![
                    LikeAtom::AnySequence,
                    LikeAtom::AnyScalar,
                    LikeAtom::Literal('%'),
                    LikeAtom::Literal('|'),
                    LikeAtom::Literal('é'),
                    LikeAtom::Literal('\u{2028}'),
                    LikeAtom::Literal('\0'),
                    LikeAtom::Literal(';'),
                    LikeAtom::Literal('~'),
                    LikeAtom::Literal('\u{2029}'),
                    LikeAtom::Literal('\\'),
                    LikeAtom::Literal('a'),
                ],
            }),
            predicate(CheckPredicate::IsNull { column: 4 }),
            predicate(CheckPredicate::IsNotNull { column: 2 }),
            predicate(CheckPredicate::In {
                column: 2,
                values: vec![
                    Value::Null,
                    Value::Text(String::from("a;")),
                    Value::Text(String::from("💾")),
                ],
            }),
            predicate(CheckPredicate::In {
                column: 0,
                values: vec![Value::Integer(i64::MIN), Value::Integer(i64::MAX)],
            }),
            predicate(CheckPredicate::In {
                column: 3,
                values: vec![Value::Boolean(false), Value::Null, Value::Boolean(true)],
            }),
        ],
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
        "~C|all_meta|AND|2|EQ|0|I-9223372036854775808|OR|2|NE|2|Tx%00007C|LT|3|B1;",
        "~C|all_meta|LE|0|I0;",
        "~C|all_meta|GT|0|I-1;",
        "~C|all_meta|GE|0|I9223372036854775807;",
        "~C|all_meta|LIKE|2|12|M|S|L%000025|L%00007C|Lé|L%002028|L%000000|L%00003B|L%00007E|L%002029|L\\|La;",
        "~C|all_meta|ISNULL|4;",
        "~C|all_meta|NOTNULL|2;",
        "~C|all_meta|IN|2|3|N|Ta%00003B|T💾;",
        "~C|all_meta|IN|0|2|I-9223372036854775808|I9223372036854775807;",
        "~C|all_meta|IN|3|3|B0|N|B1;",
    );

    let measured = measure_table_metadata(&schema, Some((0, 0))).expect("metadata measures");
    assert_eq!(measured.encoded_len(), expected.len());
    assert_eq!(
        encode_table_metadata(&schema, Some((0, 0)), measured).expect("metadata encodes"),
        expected
    );
}

#[test]
fn highly_escaped_check_like_literals_have_exact_canonical_expansion() {
    let raw = "%~|;\0\u{1f}\u{2028}\u{2029}é💾";
    let schema = TableSchema {
        name: String::from("escaped"),
        columns: vec![column("value", DataType::Text, true, None)],
        primary_key: None,
        unique_columns: Vec::new(),
        foreign_keys: Vec::new(),
        checks: vec![predicate(CheckPredicate::Like {
            column: 0,
            atoms: raw.chars().map(LikeAtom::Literal).collect(),
        })],
    };
    let expected = concat!(
        "~S|escaped|value:T:?;",
        "~C|escaped|LIKE|0|10|L%000025|L%00007E|L%00007C|L%00003B|L%000000|L%00001F|L%002028|L%002029|Lé|L💾;",
    );

    let measured = measure_table_metadata(&schema, None).expect("metadata measures");
    assert_eq!(measured.encoded_len(), expected.len());
    assert_eq!(
        encode_table_metadata(&schema, None, measured).expect("metadata encodes"),
        expected
    );
}

#[test]
fn auto_increment_measurement_matches_encoding_with_v3_check_metadata() {
    let schema = TableSchema {
        name: String::from("checked_ids"),
        columns: vec![column("id", DataType::Integer, false, None)],
        primary_key: Some(0),
        unique_columns: Vec::new(),
        foreign_keys: Vec::new(),
        checks: vec![predicate(CheckPredicate::GreaterThanOrEqual {
            column: 0,
            value: Value::Integer(0),
        })],
    };

    for last in [9, 10, i64::MAX] {
        let encoded = encode_auto_increment_record(&schema, 0, last)
            .expect("auto-increment metadata encodes");
        let prevalidated = encode_auto_increment_record_prevalidated(&schema, 0, last)
            .expect("catalog-backed auto-increment metadata encodes");
        assert_eq!(prevalidated, encoded);
        assert_eq!(
            encoded_auto_increment_record_len(&schema, 0, last)
                .expect("auto-increment metadata measures"),
            encoded.len()
        );
        assert_eq!(
            encoded_auto_increment_record_len_prevalidated(&schema, 0, last)
                .expect("catalog-backed auto-increment metadata measures"),
            encoded.len()
        );
    }
}

#[test]
fn prevalidated_auto_increment_matches_legacy_schema_bytes() {
    let schema = TableSchema {
        name: String::from("ids"),
        columns: vec![column("id", DataType::Integer, false, None)],
        primary_key: Some(0),
        unique_columns: Vec::new(),
        foreign_keys: Vec::new(),
        checks: Vec::new(),
    };

    for last in [0, 9, 10, 99, 100] {
        assert_eq!(
            encode_auto_increment_record_prevalidated(&schema, 0, last)
                .expect("catalog-backed metadata encodes"),
            encode_auto_increment_record(&schema, 0, last).expect("full metadata encodes")
        );
    }
}

#[test]
fn prevalidated_auto_increment_ignores_unrelated_invalid_check_metadata() {
    let schema = TableSchema {
        name: String::from("checked_ids"),
        columns: vec![column("id", DataType::Integer, false, None)],
        primary_key: Some(0),
        unique_columns: Vec::new(),
        foreign_keys: Vec::new(),
        checks: vec![predicate(CheckPredicate::GreaterThanOrEqual {
            column: 1,
            value: Value::Integer(0),
        })],
    };

    assert!(encode_auto_increment_record(&schema, 0, 10).is_err());
    let encoded = encode_auto_increment_record_prevalidated(&schema, 0, 10)
        .expect("valid sequence facts are sufficient on a validated catalog path");
    assert_eq!(encoded, "~A|checked_ids|id|I10;");
    assert_eq!(
        encoded_auto_increment_record_len_prevalidated(&schema, 0, 10)
            .expect("catalog-backed sequence measures"),
        encoded.len()
    );
}

#[test]
fn prevalidated_auto_increment_rejects_invalid_sequence_facts_without_panicking() {
    let integer_schema = TableSchema {
        name: String::from("items"),
        columns: vec![column("id", DataType::Integer, false, None)],
        primary_key: Some(0),
        unique_columns: Vec::new(),
        foreign_keys: Vec::new(),
        checks: Vec::new(),
    };
    assert!(encoded_auto_increment_record_len_prevalidated(&integer_schema, 1, 0).is_err());
    assert!(encoded_auto_increment_record_len_prevalidated(&integer_schema, 0, -1).is_err());

    let text_schema = TableSchema {
        name: String::from("items"),
        columns: vec![column("id", DataType::Text, false, None)],
        primary_key: Some(0),
        unique_columns: Vec::new(),
        foreign_keys: Vec::new(),
        checks: Vec::new(),
    };
    assert!(encode_auto_increment_record_prevalidated(&text_schema, 0, 0).is_err());
}
