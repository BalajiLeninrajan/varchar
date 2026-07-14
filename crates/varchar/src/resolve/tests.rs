use super::{assignments, insert_values, predicate};
use crate::sql::{Assignment, Predicate, PredicateOperator};
use crate::storage::TableSchema;
use crate::{Column, DataType, Error, Value};

fn people_schema() -> TableSchema {
    TableSchema {
        name: String::from("people"),
        columns: vec![
            Column {
                name: String::from("id"),
                data_type: DataType::Integer,
                nullable: false,
            },
            Column {
                name: String::from("note"),
                data_type: DataType::Text,
                nullable: true,
            },
            Column {
                name: String::from("active"),
                data_type: DataType::Boolean,
                nullable: false,
            },
        ],
    }
}

#[test]
fn named_insert_resolves_names_before_validating_the_row() {
    let schema = people_schema();
    assert!(matches!(
        insert_values(
            &schema,
            Some(vec![String::from("id"), String::from("missing")]),
            vec![Value::Text(String::from("wrong")), Value::Integer(1)],
        ),
        Err(Error::Schema(ref message))
            if message == "unknown column \"missing\" in table \"people\""
    ));

    assert_eq!(
        insert_values(
            &schema,
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
        .expect("named values resolve"),
        vec![
            Value::Integer(7),
            Value::Text(String::from("ready")),
            Value::Boolean(true),
        ]
    );
}

#[test]
fn assignments_validate_in_statement_order() {
    let schema = people_schema();
    let assignments_to_resolve = vec![
        Assignment {
            column: String::from("id"),
            value: Value::Text(String::from("wrong")),
        },
        Assignment {
            column: String::from("missing"),
            value: Value::Integer(1),
        },
    ];
    assert!(matches!(
        assignments(&schema, &assignments_to_resolve),
        Err(Error::Type(ref message)) if message == "column \"id\" expects INTEGER, got TEXT"
    ));
}

#[test]
fn duplicate_insert_columns_and_assignments_are_rejected() {
    let schema = people_schema();
    assert!(matches!(
        insert_values(
            &schema,
            Some(vec![String::from("id"), String::from("id")]),
            vec![Value::Integer(1), Value::Integer(2)],
        ),
        Err(Error::Schema(ref message)) if message == "duplicate INSERT column \"id\""
    ));

    let duplicate_assignments = vec![
        Assignment {
            column: String::from("id"),
            value: Value::Integer(1),
        },
        Assignment {
            column: String::from("id"),
            value: Value::Integer(2),
        },
    ];
    assert!(matches!(
        assignments(&schema, &duplicate_assignments),
        Err(Error::Schema(ref message))
            if message == "duplicate UPDATE assignment for column \"id\""
    ));
}

#[test]
fn predicate_resolution_preserves_name_and_operator_error_order() {
    let schema = people_schema();
    let missing = Predicate {
        column: String::from("missing"),
        operator: PredicateOperator::Equal(Value::Null),
    };
    assert!(matches!(
        predicate(&schema, &missing),
        Err(Error::Schema(ref message))
            if message == "unknown column \"missing\" in table \"people\""
    ));

    let null_comparison = Predicate {
        column: String::from("id"),
        operator: PredicateOperator::Equal(Value::Null),
    };
    assert!(matches!(
        predicate(&schema, &null_comparison),
        Err(Error::Type(ref message))
            if message
                == "NULL cannot be compared with `=` or `!=`; use IS NULL or IS NOT NULL"
    ));

    let wrong_like_type = Predicate {
        column: String::from("id"),
        operator: PredicateOperator::Like(String::from("anything")),
    };
    assert!(matches!(
        predicate(&schema, &wrong_like_type),
        Err(Error::Type(ref message))
            if message == "LIKE requires a TEXT column; \"id\" is INTEGER"
    ));
}
