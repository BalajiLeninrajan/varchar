use super::{parse, select};
use crate::Error;
use crate::sql::ast::{Expression, ExpressionNode};

fn expression(sql: &str) -> Expression {
    select(sql)
        .where_clause
        .expect("fixture SELECT has a WHERE expression")
}

fn predicate_name(node: &ExpressionNode) -> &str {
    let ExpressionNode::Predicate(predicate) = node else {
        panic!("expected predicate node, got {node:?}");
    };
    &predicate.column.name
}

fn assert_unsupported(sql: &str, expected_feature: &str, marker: &str) {
    let span_start = sql.find(marker).expect("fixture contains error marker");
    let span_end = span_start + marker.len();
    match parse(sql) {
        Err(Error::Unsupported {
            feature,
            span_start: actual_start,
            span_end: actual_end,
        }) => {
            assert_eq!(feature, expected_feature, "feature for {sql:?}");
            assert_eq!(
                (actual_start, actual_end),
                (span_start, span_end),
                "span for {sql:?}"
            );
        }
        other => panic!("expected exact Unsupported error for {sql:?}, got {other:?}"),
    }
}

#[test]
fn parentheses_override_precedence_and_associative_nodes_flatten() {
    let flattened = expression("SELECT * FROM t WHERE (a = 1 AND b = 2) AND (c = 3 AND d = 4)");
    assert!(matches!(
        flattened.nodes()[0],
        ExpressionNode::And { children: 4 }
    ));
    assert_eq!(flattened.nodes().len(), 5);
    assert_eq!(
        flattened
            .nodes()
            .iter()
            .skip(1)
            .map(predicate_name)
            .collect::<Vec<_>>(),
        ["a", "b", "c", "d"]
    );
}

#[test]
fn malformed_supported_expressions_are_parse_errors() {
    for sql in [
        "SELECT * FROM t WHERE (a = 1",
        "SELECT * FROM t WHERE a = 1)",
        "SELECT * FROM t WHERE ()",
        "SELECT * FROM t WHERE a = 1 AND",
        "SELECT * FROM t WHERE a = AND b = 2",
    ] {
        assert!(
            matches!(parse(sql), Err(Error::Parse { .. })),
            "expected Parse for {sql:?}"
        );
    }
}

#[test]
fn excluded_expression_forms_have_structured_features_and_exact_spans() {
    for (sql, feature, marker) in [
        ("SELECT * FROM t WHERE NOT a = 1", "unary NOT", "NOT"),
        (
            "SELECT * FROM t WHERE TRUE",
            "bare Boolean constants",
            "TRUE",
        ),
        (
            "SELECT * FROM t WHERE FALSE",
            "bare Boolean constants",
            "FALSE",
        ),
        (
            "SELECT * FROM t WHERE 1 = 1",
            "literal-to-literal predicates",
            "1",
        ),
        (
            "SELECT * FROM t WHERE NULL",
            "literal-to-literal predicates",
            "NULL",
        ),
        (
            "SELECT * FROM t WHERE a = b",
            "column-to-column WHERE predicates",
            "b",
        ),
    ] {
        assert_unsupported(sql, feature, marker);
    }
}
