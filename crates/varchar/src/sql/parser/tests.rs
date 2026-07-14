use super::parse;
use crate::sql::ast::{Predicate, PredicateOperator, Projection, Select, Statement};
use crate::{Error, Value};

#[test]
fn parsing_produces_the_exact_normalized_ast() {
    assert_eq!(
        parse("SeLeCt Name, ID FROM Users WHERE Name LIKE 'a_%' AND ID != -7;")
            .expect("SELECT parses"),
        Statement::Select(Select {
            table: String::from("users"),
            projection: Projection::Columns(vec![String::from("name"), String::from("id"),]),
            predicates: vec![
                Predicate {
                    column: String::from("name"),
                    operator: PredicateOperator::Like(String::from("a_%")),
                },
                Predicate {
                    column: String::from("id"),
                    operator: PredicateOperator::NotEqual(Value::Integer(-7)),
                },
            ],
        })
    );
}

#[test]
fn unsupported_trailing_syntax_keeps_its_feature_and_span() {
    assert!(matches!(
        parse("SELECT * FROM t JOIN u"),
        Err(Error::Unsupported {
            ref feature,
            span_start: 16,
            span_end: 20,
        }) if feature == "joins"
    ));
}
