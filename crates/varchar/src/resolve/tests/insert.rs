use super::people_schema;
use crate::resolve::insert_values;
use crate::storage::{AutoIncrement, TableSchema};
use crate::{DataType, Error, SchemaColumn, Value};

#[test]
fn auto_increment_resolution_generates_and_tracks_only_new_high_water_marks() {
    let schema = TableSchema {
        name: String::from("ids"),
        columns: vec![SchemaColumn {
            name: String::from("id"),
            data_type: DataType::Integer,
            nullable: false,
            default: None,
        }],
        primary_key: Some(0),
        unique_columns: Vec::new(),
        foreign_keys: Vec::new(),
        checks: Vec::new(),
    };
    let auto_increment = Some(AutoIncrement { column: 0, last: 4 });

    let generated = insert_values(&schema, auto_increment, None, vec![Value::Null])
        .expect("NULL generates a value");
    assert_eq!(generated.values, vec![Value::Integer(5)]);
    assert_eq!(generated.next_auto_increment, Some(5));

    let explicit_lower = insert_values(&schema, auto_increment, None, vec![Value::Integer(-1)])
        .expect("an explicit lower value is retained");
    assert_eq!(explicit_lower.values, vec![Value::Integer(-1)]);
    assert_eq!(explicit_lower.next_auto_increment, None);
}

#[test]
fn sequence_exhaustion_precedes_remaining_value_validation() {
    let schema = TableSchema {
        name: String::from("ids"),
        columns: vec![
            SchemaColumn {
                name: String::from("id"),
                data_type: DataType::Integer,
                nullable: false,
                default: None,
            },
            SchemaColumn {
                name: String::from("required"),
                data_type: DataType::Text,
                nullable: false,
                default: None,
            },
        ],
        primary_key: Some(0),
        unique_columns: Vec::new(),
        foreign_keys: Vec::new(),
        checks: Vec::new(),
    };

    assert!(matches!(
        insert_values(
            &schema,
            Some(AutoIncrement {
                column: 0,
                last: i64::MAX,
            }),
            None,
            vec![Value::Null, Value::Null],
        ),
        Err(Error::Constraint(ref message))
            if message == "auto-increment sequence for table \"ids\" is exhausted"
    ));
}

#[test]
fn named_insert_resolves_names_before_validating_the_row() {
    let schema = people_schema();
    assert!(matches!(
        insert_values(
            &schema,
            None,
            Some(vec![String::from("id"), String::from("missing")]),
            vec![Value::Text(String::from("wrong")), Value::Integer(1)],
        ),
        Err(Error::Schema(ref message))
            if message == "unknown column \"missing\" in table \"people\""
    ));

    assert_eq!(
        insert_values(
            &schema,
            None,
            Some(vec![
                String::from("active"),
                String::from("id"),
                String::from("note"),
            ]),
            vec![
                Value::Boolean(true),
                Value::Integer(7),
                Value::Text(String::from("ready")),
            ],
        )
        .expect("named values resolve")
        .values,
        vec![
            Value::Integer(7),
            Value::Text(String::from("ready")),
            Value::Boolean(true),
        ]
    );
}

#[test]
fn duplicate_insert_columns_are_rejected() {
    let schema = people_schema();
    assert!(matches!(
        insert_values(
            &schema,
            None,
            Some(vec![String::from("id"), String::from("id")]),
            vec![Value::Integer(1), Value::Integer(2)],
        ),
        Err(Error::Schema(ref message)) if message == "duplicate INSERT column \"id\""
    ));
}
