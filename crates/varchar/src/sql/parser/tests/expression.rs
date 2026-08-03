use super::{parse, select};
use crate::Error;
use crate::sql::ast::{Expression, ExpressionNode, Statement};

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
fn and_binds_more_tightly_than_or() {
    let expression = expression("SELECT * FROM t WHERE a = 1 OR b = 2 AND c = 3");
    assert!(matches!(
        expression.nodes()[0],
        ExpressionNode::Or { children: 2 }
    ));
    assert_eq!(predicate_name(&expression.nodes()[1]), "a");
    assert!(matches!(
        expression.nodes()[2],
        ExpressionNode::And { children: 2 }
    ));
    assert_eq!(predicate_name(&expression.nodes()[3]), "b");
    assert_eq!(predicate_name(&expression.nodes()[4]), "c");
    assert_eq!(expression.to_string(), "a = 1 OR b = 2 AND c = 3");
}

#[test]
fn parentheses_override_precedence_and_associative_nodes_flatten() {
    let grouped = expression("SELECT * FROM t WHERE (a = 1 OR b = 2) AND c = 3");
    assert!(matches!(
        grouped.nodes()[0],
        ExpressionNode::And { children: 2 }
    ));
    assert!(matches!(
        grouped.nodes()[1],
        ExpressionNode::Or { children: 2 }
    ));
    assert_eq!(grouped.to_string(), "(a = 1 OR b = 2) AND c = 3");

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
        "SELECT * FROM t WHERE a = 1 OR OR b = 2",
        "SELECT * FROM t WHERE a = AND b = 2",
    ] {
        assert!(
            matches!(parse(sql), Err(Error::Parse { .. })),
            "expected Parse for {sql:?}"
        );
    }
}

#[test]
fn recognized_trailing_features_remain_statement_level_unsupported_errors() {
    for (sql, feature, marker) in [
        (
            "SELECT * FROM t WHERE id = 1 AND (id = 2 OR id = 3) ORDER BY id",
            "ORDER BY",
            "ORDER",
        ),
        (
            "EXPLAIN REGEX SELECT * FROM t WHERE (id = 1 OR id = 2) GROUP BY id",
            "GROUP BY",
            "GROUP",
        ),
        (
            "UPDATE t SET id = 1 WHERE (id = 1 OR id = 2) LIMIT 1",
            "LIMIT",
            "LIMIT",
        ),
        (
            "DELETE FROM t WHERE id = 1 AND (id = 2 OR id = 3) AS alias",
            "aliases",
            "AS",
        ),
    ] {
        assert_unsupported(sql, feature, marker);
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

#[test]
fn deep_parse_format_and_destruction_use_explicit_stacks() {
    const DEPTH: usize = 2_000;
    let mut sql = String::from("SELECT * FROM t WHERE ");
    sql.push_str(&"(".repeat(DEPTH));
    sql.push_str("a = 1");
    for index in 0..DEPTH {
        if index % 2 == 0 {
            sql.push_str(" AND a = 1)");
        } else {
            sql.push_str(" OR a = 1)");
        }
    }

    let Statement::Select(statement) = parse(&sql).expect("deep expression parses") else {
        panic!("expected SELECT");
    };
    let expression = statement.where_clause.expect("WHERE exists");
    assert_eq!(expression.predicate_units().expect("count fits"), DEPTH + 1);
    let formatted = expression.to_string();
    assert!(formatted.starts_with("("));
    drop(formatted);
    drop(expression);
}
