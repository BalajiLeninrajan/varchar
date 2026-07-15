use super::people_schema;
use crate::resolve::predicate;
use crate::sql::{Predicate, PredicateOperator};
use crate::{Error, Value};

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
