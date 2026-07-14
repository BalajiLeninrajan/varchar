use super::{assert_error, assert_resource_error, catalog, people_schema, select_statement};
use crate::resolve::{predicate, select};
use crate::sql::{ColumnRef, Predicate, PredicateOperator};
use crate::{ErrorCode, Resource, Value};

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
    assert_error(
        predicate(&schema, &missing),
        ErrorCode::Schema,
        "unknown column \"missing\" in table \"people\"",
    );

    let null_comparison = Predicate {
        column: ColumnRef {
            qualifier: None,
            name: String::from("id"),
        },
        operator: PredicateOperator::Equal(Value::Null),
    };
    assert_error(
        predicate(&schema, &null_comparison),
        ErrorCode::Type,
        "NULL cannot be compared with `=` or `!=`; use IS NULL or IS NOT NULL",
    );

    let wrong_like_type = Predicate {
        column: ColumnRef {
            qualifier: None,
            name: String::from("id"),
        },
        operator: PredicateOperator::Like(String::from("anything")),
    };
    assert_error(
        predicate(&schema, &wrong_like_type),
        ErrorCode::Type,
        "LIKE requires a TEXT column; \"id\" is INTEGER",
    );
}

#[test]
fn select_predicates_resolve_in_statement_order() {
    let catalog = catalog("V2;~S|t|id:I:!|note:T:!;");
    let invalid_like_first =
        select_statement(r"SELECT id FROM t WHERE note LIKE 'bad\q' AND missing = 1");
    assert_error(
        select(&catalog, &invalid_like_first, 4, 4, usize::MAX),
        ErrorCode::Type,
        "LIKE pattern contains unsupported escape \\q",
    );

    let missing_first =
        select_statement(r"SELECT id FROM t WHERE missing = 1 AND note LIKE 'bad\q'");
    assert_error(
        select(&catalog, &missing_first, 4, 4, usize::MAX),
        ErrorCode::Schema,
        "unknown column \"missing\" in table \"t\"",
    );
}

#[test]
fn select_predicate_limit_is_enforced() {
    let catalog = catalog("V2;~S|t|id:I:!|note:T:!;");
    let statement = select_statement("SELECT id FROM t WHERE id = 1 AND note = 'one'");

    assert_resource_error(
        select(&catalog, &statement, 1, 1, usize::MAX),
        Resource::WherePredicates,
        1,
    );
}
