use super::people_schema;
use crate::resolve::assignments;
use crate::sql::Assignment;
use crate::{Error, Value};

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
        assignments(&schema, None, &assignments_to_resolve),
        Err(Error::Type(ref message))
            if message == "column \"id\" expects INTEGER, got TEXT"
    ));
}

#[test]
fn duplicate_assignments_are_rejected() {
    let schema = people_schema();
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
        assignments(&schema, None, &duplicate_assignments),
        Err(Error::Schema(ref message))
            if message == "duplicate UPDATE assignment for column \"id\""
    ));
}
