use super::{catalog, people_schema, select_statement};
use crate::resolve::{ResolvedPredicate, predicate, select};
use crate::sql::{ColumnRef, Predicate, PredicateOperator};
use crate::{Error, Resource, Value};

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
fn ordered_predicates_require_same_type_non_null_scalars() {
    let schema = people_schema();
    for (name, operator) in [
        ("id", PredicateOperator::LessThan(Value::Integer(10))),
        (
            "note",
            PredicateOperator::LessThanOrEqual(Value::Text(String::from("m"))),
        ),
        (
            "active",
            PredicateOperator::GreaterThan(Value::Boolean(false)),
        ),
        (
            "id",
            PredicateOperator::GreaterThanOrEqual(Value::Integer(1)),
        ),
    ] {
        let parsed = Predicate {
            column: ColumnRef {
                qualifier: None,
                name: name.to_owned(),
            },
            operator,
        };
        predicate(&schema, &parsed).expect("same-type ordered predicate resolves");
    }

    for operator in [
        PredicateOperator::LessThan(Value::Null),
        PredicateOperator::LessThanOrEqual(Value::Null),
        PredicateOperator::GreaterThan(Value::Null),
        PredicateOperator::GreaterThanOrEqual(Value::Null),
    ] {
        let null_ordering = Predicate {
            column: ColumnRef {
                qualifier: None,
                name: String::from("id"),
            },
            operator,
        };
        assert!(matches!(
            predicate(&schema, &null_ordering),
            Err(Error::Type(ref message))
                if message
                    == "NULL cannot be compared with `<`, `<=`, `>`, or `>=`; use IS NULL or IS NOT NULL"
        ));
    }

    let wrong_type = Predicate {
        column: ColumnRef {
            qualifier: None,
            name: String::from("active"),
        },
        operator: PredicateOperator::GreaterThan(Value::Integer(0)),
    };
    assert!(matches!(
        predicate(&schema, &wrong_type),
        Err(Error::Type(ref message))
            if message == "column \"active\" expects BOOLEAN, got INTEGER"
    ));
}

#[test]
fn in_resolves_every_member_left_to_right_and_allows_untyped_null() {
    let schema = people_schema();
    for name in ["id", "note", "active"] {
        let all_null = Predicate {
            column: ColumnRef {
                qualifier: None,
                name: name.to_owned(),
            },
            operator: PredicateOperator::In(vec![Value::Null, Value::Null]),
        };
        assert!(matches!(
            predicate(&schema, &all_null),
            Ok(ResolvedPredicate::In { values, .. })
                if values == [Value::Null, Value::Null]
        ));
    }

    let duplicates = Predicate {
        column: ColumnRef {
            qualifier: None,
            name: String::from("id"),
        },
        operator: PredicateOperator::In(vec![Value::Integer(1), Value::Integer(1), Value::Null]),
    };
    assert!(matches!(
        predicate(&schema, &duplicates),
        Ok(ResolvedPredicate::In { values, .. })
            if values == [Value::Integer(1), Value::Integer(1), Value::Null]
    ));

    let bad_later_member = Predicate {
        column: ColumnRef {
            qualifier: None,
            name: String::from("id"),
        },
        operator: PredicateOperator::In(vec![
            Value::Integer(1),
            Value::Text(String::from("wrong")),
            Value::Boolean(false),
        ]),
    };
    assert!(matches!(
        predicate(&schema, &bad_later_member),
        Err(Error::Type(ref message))
            if message == "column \"id\" expects INTEGER, got TEXT"
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
            resource: Resource::WherePredicates,
            limit: 1,
        })
    ));
}
