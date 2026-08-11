use super::{parse, select};
use crate::sql::ast::{Expression, ExpressionNode, PredicateOperator, Statement};
use crate::{Error, Value};

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

fn assert_parse_error(sql: &str, expected_message: &str, marker: &str) {
    let span_start = sql.find(marker).expect("fixture contains error marker");
    let span_end = span_start + marker.len();
    match parse(sql) {
        Err(Error::Parse {
            message,
            span_start: actual_start,
            span_end: actual_end,
        }) => {
            assert_eq!(message, expected_message, "message for {sql:?}");
            assert_eq!(
                (actual_start, actual_end),
                (span_start, span_end),
                "span for {sql:?}"
            );
        }
        other => panic!("expected exact Parse error for {sql:?}, got {other:?}"),
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
fn ordered_and_membership_predicates_stay_flat_and_format_canonically() {
    let expression = expression(
        "SELECT * FROM t WHERE a < 1 AND b <= 2 AND c > 3 AND d >= 4 AND e IN ('x', NULL, 'x')",
    );

    assert!(matches!(
        expression.nodes()[0],
        ExpressionNode::And { children: 5 }
    ));
    assert!(matches!(
        &expression.nodes()[1],
        ExpressionNode::Predicate(predicate)
            if predicate.operator == PredicateOperator::LessThan(Value::Integer(1))
    ));
    assert!(matches!(
        &expression.nodes()[2],
        ExpressionNode::Predicate(predicate)
            if predicate.operator == PredicateOperator::LessThanOrEqual(Value::Integer(2))
    ));
    assert!(matches!(
        &expression.nodes()[3],
        ExpressionNode::Predicate(predicate)
            if predicate.operator == PredicateOperator::GreaterThan(Value::Integer(3))
    ));
    assert!(matches!(
        &expression.nodes()[4],
        ExpressionNode::Predicate(predicate)
            if predicate.operator == PredicateOperator::GreaterThanOrEqual(Value::Integer(4))
    ));
    assert!(matches!(
        &expression.nodes()[5],
        ExpressionNode::Predicate(predicate)
            if predicate.operator
                == PredicateOperator::In(vec![
                    Value::Text(String::from("x")),
                    Value::Null,
                    Value::Text(String::from("x")),
                ])
    ));
    assert_eq!(expression.predicate_units().expect("count fits"), 7);
    assert_eq!(
        expression.to_string(),
        "a < 1 AND b <= 2 AND c > 3 AND d >= 4 AND e IN ('x', NULL, 'x')"
    );
}

#[test]
fn empty_in_is_excluded_while_all_null_lists_are_valid() {
    let sql = "SELECT * FROM t WHERE value IN ()";
    assert!(matches!(
        parse(sql),
        Err(Error::Unsupported {
            ref feature,
            span_start: 28,
            span_end: 30,
        }) if feature == "empty IN lists"
    ));

    let expression = expression("SELECT * FROM t WHERE value IN (NULL, NULL)");
    assert_eq!(expression.predicate_units().expect("count fits"), 2);
    assert_eq!(expression.to_string(), "value IN (NULL, NULL)");

    assert!(matches!(
        parse("SELECT * FROM t WHERE value IN (1, )"),
        Err(Error::Parse { .. })
    ));
    assert!(matches!(
        parse("SELECT * FROM t WHERE value IN (SELECT id FROM t)"),
        Err(Error::Unsupported {
            ref feature,
            span_start: 32,
            span_end: 38,
        }) if feature == "subqueries in IN lists"
    ));
    assert!(matches!(
        parse("SELECT * FROM t WHERE value IN (1 = 1)"),
        Err(Error::Unsupported {
            ref feature,
            span_start: 34,
            span_end: 35,
        }) if feature == "expressions in IN lists"
    ));
    assert_unsupported(
        "SELECT * FROM t WHERE value IN (1 BETWEEN 0 AND 2)",
        "expressions in IN lists",
        "BETWEEN",
    );
    assert_unsupported(
        "SELECT * FROM t WHERE value IN (1 + 2)",
        "expressions in IN lists",
        "+",
    );
    assert_unsupported(
        "SELECT * FROM t WHERE value IN (\"other\")",
        "expressions in IN lists",
        "\"other\"",
    );
}

#[test]
fn in_is_reserved_and_cannot_be_used_as_an_identifier() {
    for (sql, marker) in [
        ("CREATE TABLE in (id INTEGER)", "in"),
        ("CREATE TABLE t (in INTEGER)", "in"),
        ("SELECT in FROM t", "in"),
        ("SELECT * FROM in", "in"),
        ("SELECT * FROM t WHERE in = 1", "in"),
        ("INSERT INTO in (id) VALUES (1)", "in"),
        ("UPDATE in SET id = 1", "in"),
        ("DELETE FROM in", "in"),
    ] {
        assert_parse_error(
            sql,
            "reserved keyword `IN` cannot be used as an identifier",
            marker,
        );
    }
}

#[test]
fn in_still_drives_the_membership_predicate_after_reservation() {
    let membership = expression("SELECT * FROM t WHERE value IN (1)");
    assert_eq!(membership.to_string(), "value IN (1)");

    assert!(matches!(
        parse("SELECT * FROM t WHERE value IN"),
        Err(Error::Parse {
            ref message,
            span_start: 28,
            span_end: 30,
        }) if message.starts_with("expected `=`, `!=`")
    ));
}

#[test]
fn malformed_comparison_operators_are_rejected_as_units_by_the_parser() {
    for operator in ["==", "=>", "<<", ">>", "!<", "!==", "<==", "><", "<>="] {
        let sql = format!("SELECT * FROM t WHERE a {operator} 1");
        assert_parse_error(
            &sql,
            &format!("malformed comparison operator `{operator}`"),
            operator,
        );
    }

    assert_parse_error("SELECT * FROM t WHERE a ! 1", "expected `=` after `!`", "!");
    assert_unsupported(
        "SELECT * FROM t WHERE a <> 1",
        "comparison operator `<>`",
        "<>",
    );
}

#[test]
fn deferred_lexical_errors_surface_with_their_original_diagnostics() {
    assert_parse_error(
        "SELECT * FROM t WHERE a = '\u{e9}",
        "unterminated string literal",
        "'\u{e9}",
    );
    assert_parse_error(
        "SELECT \u{1f4a5} FROM t",
        "unexpected character '\u{1f4a5}'",
        "\u{1f4a5}",
    );
    // Quoted identifiers are a supported token now; only an unterminated one
    // still reaches the parser as a deferred lexical error.
    assert_parse_error(
        "SELECT \"a FROM t",
        "unterminated quoted identifier",
        "\"a FROM t",
    );
    assert_unsupported("SELECT a FROM t -- tail", "SQL comments", "-- tail");
    assert_unsupported("SELECT a FROM t /* tail", "SQL comments", "/* tail");
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
