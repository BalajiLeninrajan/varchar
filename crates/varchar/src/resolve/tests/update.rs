use super::{assert_error, people_schema};
use crate::resolve::assignments;
use crate::sql::Assignment;
use crate::{ErrorCode, Value};

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
    assert_error(
        assignments(&schema, None, &assignments_to_resolve),
        ErrorCode::Type,
        "column \"id\" expects INTEGER, got TEXT",
    );
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
    assert_error(
        assignments(&schema, None, &duplicate_assignments),
        ErrorCode::Schema,
        "duplicate UPDATE assignment for column \"id\"",
    );
}
