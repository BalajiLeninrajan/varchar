use super::{catalog, people_schema, select_statement};
use crate::resolve::{predicate, select};
use crate::sql::{ColumnRef, Predicate, PredicateOperator};
use crate::{Error, Value};

#[test]
fn predicate_resolution_preserves_name_and_operator_error_order() {
    let schema = people_schema();
    let missing = Predicate {
        column: ColumnRef {
            qualifier: None,
            name: String::from("missing"),
        },
        operator: PredicateOperator::Equal(Value::Null),
    };
    assert!(matches!(
        predicate(&schema, &missing),
        Err(Error::Schema(ref message))
            if message == "unknown column \"missing\" in table \"people\""
    ));

    let null_comparison = Predicate {
        column: ColumnRef {
            qualifier: None,
            name: String::from("id"),
        },
        operator: PredicateOperator::Equal(Value::Null),
    };
    assert!(matches!(
        predicate(&schema, &null_comparison),
        Err(Error::Type(ref message))
            if message
                == "NULL cannot be compared with `=` or `!=`; use IS NULL or IS NOT NULL"
    ));

    let wrong_like_type = Predicate {
        column: ColumnRef {
            qualifier: None,
            name: String::from("id"),
        },
        operator: PredicateOperator::Like(String::from("anything")),
    };
    assert!(matches!(
        predicate(&schema, &wrong_like_type),
        Err(Error::Type(ref message))
            if message == "LIKE requires a TEXT column; \"id\" is INTEGER"
    ));
}

#[test]
fn select_predicates_resolve_in_statement_order() {
    let catalog = catalog("V2;~S|t|id:I:!|note:T:!;");
    let invalid_like_first =
        select_statement(r"SELECT id FROM t WHERE note LIKE 'bad\q' AND missing = 1");
    assert!(matches!(
        select(&catalog, &invalid_like_first, 4, 4, usize::MAX),
        Err(Error::Type(ref message))
            if message == "LIKE pattern contains unsupported escape \\q"
    ));

    let missing_first =
        select_statement(r"SELECT id FROM t WHERE missing = 1 AND note LIKE 'bad\q'");
    assert!(matches!(
        select(&catalog, &missing_first, 4, 4, usize::MAX),
        Err(Error::Schema(ref message))
            if message == "unknown column \"missing\" in table \"t\""
    ));
}

#[test]
fn select_predicate_limit_is_enforced() {
    let catalog = catalog("V2;~S|t|id:I:!|note:T:!;");
    let statement = select_statement("SELECT id FROM t WHERE id = 1 AND note = 'one'");

    assert!(matches!(
        select(&catalog, &statement, 1, 1, usize::MAX),
        Err(Error::ResourceLimit {
            resource: "WHERE predicates",
            limit: 1,
        })
    ));
}
