use super::{column_ref, parse, select};
use crate::Error;
use crate::sql::ast::{OrderDirection, OrderTerm};

fn assert_unsupported(sql: &str, expected_feature: &str, marker: &str) {
    let span_start = sql.rfind(marker).expect("fixture contains error marker");
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

fn assert_missing_comma(sql: &str, marker: &str) {
    let span_start = sql.rfind(marker).expect("fixture contains error marker");
    let span_end = span_start + marker.len();
    match parse(sql) {
        Err(Error::Parse {
            message,
            span_start: actual_start,
            span_end: actual_end,
        }) => {
            assert_eq!(message, "expected `,` between ORDER BY terms");
            assert_eq!(
                (actual_start, actual_end),
                (span_start, span_end),
                "span for {sql:?}"
            );
        }
        other => panic!("expected exact Parse error for {sql:?}, got {other:?}"),
    }
}

fn assert_reserved(sql: &str, keyword: &str, marker: &str) {
    let span_start = sql.find(marker).expect("fixture contains error marker");
    let span_end = span_start + marker.len();
    match parse(sql) {
        Err(Error::Parse {
            message,
            span_start: actual_start,
            span_end: actual_end,
        }) => {
            assert_eq!(
                message,
                format!("reserved keyword `{keyword}` cannot be used as an identifier"),
                "message for {sql:?}"
            );
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
fn parses_source_column_terms_and_directions_in_order() {
    let statement = select(
        "SELECT users.name FROM users WHERE users.active = TRUE \
         ORDER BY users.created_at DESC, name, active ASC, name DESC",
    );

    assert_eq!(
        statement.order_by,
        vec![
            OrderTerm {
                column: column_ref(Some("users"), "created_at"),
                direction: OrderDirection::Descending,
            },
            OrderTerm {
                column: column_ref(None, "name"),
                direction: OrderDirection::Ascending,
            },
            OrderTerm {
                column: column_ref(None, "active"),
                direction: OrderDirection::Ascending,
            },
            OrderTerm {
                column: column_ref(None, "name"),
                direction: OrderDirection::Descending,
            },
        ]
    );
}

#[test]
fn asc_and_desc_are_reserved_and_cannot_be_used_as_identifiers() {
    for (sql, keyword, marker) in [
        ("CREATE TABLE asc (id INTEGER)", "ASC", "asc"),
        ("CREATE TABLE directions (desc TEXT)", "DESC", "desc"),
        ("SELECT asc FROM directions", "ASC", "asc"),
        ("SELECT desc FROM directions", "DESC", "desc"),
        ("SELECT id FROM asc", "ASC", "asc"),
        ("SELECT id FROM directions WHERE desc = 'x'", "DESC", "desc"),
        ("SELECT id FROM directions ORDER BY asc", "ASC", "asc"),
        (
            "SELECT id FROM directions ORDER BY desc ASC",
            "DESC",
            "desc",
        ),
        ("INSERT INTO desc (id) VALUES (1)", "DESC", "desc"),
        ("UPDATE directions SET asc = 1", "ASC", "asc"),
        ("DELETE FROM desc", "DESC", "desc"),
    ] {
        assert_reserved(sql, keyword, marker);
    }
}

// `OFFSET` was a contextual identifier until the pagination tail introduced it
// as a keyword; its reservation is covered by the pagination parser tests.

#[test]
fn excluded_ordering_forms_have_order_specific_features_and_operator_spans() {
    for (sql, feature, marker) in [
        ("SELECT id FROM t ORDER BY 1", "ORDER BY ordinals", "1"),
        (
            "SELECT id FROM t ORDER BY (id + 1)",
            "ORDER BY expressions",
            "(",
        ),
        (
            "SELECT id FROM t ORDER BY id = 1",
            "ORDER BY expressions",
            "=",
        ),
        (
            "SELECT id FROM t ORDER BY id != 1",
            "ORDER BY expressions",
            "!=",
        ),
        (
            "SELECT id FROM t ORDER BY id < 1",
            "ORDER BY expressions",
            "<",
        ),
        (
            "SELECT id FROM t ORDER BY id <= 1",
            "ORDER BY expressions",
            "<=",
        ),
        (
            "SELECT id FROM t ORDER BY id > 1",
            "ORDER BY expressions",
            ">",
        ),
        (
            "SELECT id FROM t ORDER BY id >= 1",
            "ORDER BY expressions",
            ">=",
        ),
        (
            "SELECT id FROM t ORDER BY id <> 1",
            "ORDER BY expressions",
            "<>",
        ),
        (
            "SELECT id FROM t ORDER BY id IN (1)",
            "ORDER BY expressions",
            "IN",
        ),
        (
            "SELECT id FROM t ORDER BY id LIKE '1'",
            "ORDER BY expressions",
            "LIKE",
        ),
        (
            "SELECT id FROM t ORDER BY id IS NULL",
            "ORDER BY expressions",
            "IS",
        ),
        (
            "SELECT id FROM t ORDER BY id BETWEEN 0 AND 1",
            "ORDER BY expressions",
            "BETWEEN",
        ),
        (
            "SELECT id FROM t ORDER BY id + 1",
            "ORDER BY expressions",
            "+",
        ),
        (
            "SELECT id FROM t ORDER BY id-1",
            "ORDER BY expressions",
            "-",
        ),
        (
            "SELECT id FROM t ORDER BY id / 1",
            "ORDER BY expressions",
            "/",
        ),
        (
            "SELECT id FROM t ORDER BY id % 1",
            "ORDER BY expressions",
            "%",
        ),
        (
            "SELECT id FROM t ORDER BY id | 1",
            "ORDER BY expressions",
            "|",
        ),
        (
            "SELECT id FROM t ORDER BY id * 1",
            "ORDER BY expressions",
            "*",
        ),
        (
            "SELECT id FROM t ORDER BY ABS(id)",
            "ORDER BY expressions",
            "(",
        ),
        (
            "SELECT id FROM t ORDER BY id COLLATE binary",
            "ORDER BY COLLATE",
            "COLLATE",
        ),
        (
            "SELECT id FROM t ORDER BY id NULLS FIRST",
            "ORDER BY NULLS FIRST/LAST",
            "NULLS",
        ),
    ] {
        assert_unsupported(sql, feature, marker);
    }
}

#[test]
fn lexical_errors_after_order_terms_keep_their_original_diagnostics() {
    let sql = "SELECT id FROM t ORDER BY id -- comment";
    let span_start = sql.find("--").expect("fixture contains SQL comment");
    match parse(sql) {
        Err(Error::Unsupported {
            feature,
            span_start: actual_start,
            span_end: actual_end,
        }) => {
            assert_eq!(feature, "SQL comments");
            assert_eq!((actual_start, actual_end), (span_start, sql.len()));
        }
        other => panic!("expected SQL-comment diagnostic, got {other:?}"),
    }

    let sql = "SELECT id FROM t ORDER BY id \"x\"";
    let span_start = sql.find('"').expect("fixture contains quoted identifier");
    match parse(sql) {
        Err(Error::Unsupported {
            feature,
            span_start: actual_start,
            span_end: actual_end,
        }) => {
            assert_eq!(feature, "quoted identifiers");
            assert_eq!((actual_start, actual_end), (span_start, span_start + 1));
        }
        other => panic!("expected quoted-identifier diagnostic, got {other:?}"),
    }

    let sql = "SELECT id FROM t ORDER BY id @";
    let span_start = sql
        .find('@')
        .expect("fixture contains unexpected character");
    match parse(sql) {
        Err(Error::Parse {
            message,
            span_start: actual_start,
            span_end: actual_end,
        }) => {
            assert_eq!(message, "unexpected character '@'");
            assert_eq!((actual_start, actual_end), (span_start, span_start + 1));
        }
        other => panic!("expected unexpected-character diagnostic, got {other:?}"),
    }
}

#[test]
fn malformed_order_by_is_a_parse_error() {
    for sql in [
        "SELECT id FROM t ORDER id",
        "SELECT id FROM t ORDER BY",
        "SELECT id FROM t ORDER BY id,",
        "SELECT id FROM t ORDER BY t.",
    ] {
        assert!(
            matches!(parse(sql), Err(Error::Parse { .. })),
            "expected Parse for {sql:?}"
        );
    }

    for (sql, marker) in [
        ("SELECT id FROM t ORDER BY id name", "name"),
        ("SELECT id FROM t ORDER BY id DESC name ASC", "name"),
        ("SELECT id FROM t ORDER BY id other.name", "other"),
    ] {
        assert_missing_comma(sql, marker);
    }
}
