use super::{parse, select};
use crate::Error;

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
fn parses_limit_offset_and_the_complete_u64_range() {
    let statement = select(
        "SELECT id FROM events ORDER BY created_at DESC \
         LIMIT 00025 OFFSET 18446744073709551615;",
    );
    assert_eq!(statement.limit, Some(25));
    assert_eq!(statement.offset, Some(u64::MAX));

    let statement = select("SELECT * FROM events OFFSET 0000000000000000000000000000000000000000");
    assert_eq!(statement.limit, None);
    assert_eq!(statement.offset, Some(0));

    let statement = select("SELECT * FROM events ORDER BY id OFFSET 18446744073709551615");
    assert_eq!(statement.limit, None);
    assert_eq!(statement.offset, Some(u64::MAX));

    let statement = select("SELECT * FROM events LIMIT 0");
    assert_eq!(statement.limit, Some(0));
    assert_eq!(statement.offset, None);
}

#[test]
fn offset_is_reserved_and_cannot_be_used_as_an_identifier() {
    for sql in [
        "CREATE TABLE offset (id INTEGER)",
        "CREATE TABLE pages (offset INTEGER)",
        "INSERT INTO offset (id) VALUES (1)",
        "SELECT offset FROM pages",
        "SELECT id FROM offset",
        "SELECT id FROM pages WHERE offset = 1",
        "SELECT id FROM pages ORDER BY offset DESC",
        "UPDATE pages SET offset = 1",
        "DELETE FROM offset",
    ] {
        assert_reserved(sql, "OFFSET", "offset");
    }

    // The keyword still drives the pagination clause it was reserved for.
    let statement = select("SELECT id FROM pages ORDER BY id LIMIT 1 OFFSET 2");
    assert_eq!(statement.limit, Some(1));
    assert_eq!(statement.offset, Some(2));
}

#[test]
fn malformed_pagination_values_are_parse_errors() {
    for sql in [
        "SELECT * FROM t LIMIT -0",
        "SELECT * FROM t OFFSET -1",
        "SELECT * FROM t LIMIT +1",
        "SELECT * FROM t LIMIT 1.0",
        "SELECT * FROM t OFFSET '1'",
        "SELECT * FROM t LIMIT TRUE",
        "SELECT * FROM t LIMIT 18446744073709551616",
        "SELECT * FROM t LIMIT",
        "SELECT * FROM t OFFSET",
    ] {
        assert!(
            matches!(parse(sql), Err(Error::Parse { .. })),
            "expected Parse for {sql:?}"
        );
    }
}

#[test]
fn glued_numeric_tokens_report_the_complete_span() {
    assert!(matches!(
        parse("SELECT * FROM t LIMIT 1foo"),
        Err(Error::Parse {
            ref message,
            span_start: 22,
            span_end: 26,
        }) if message == "malformed numeric token"
    ));
    assert!(matches!(
        parse("SELECT * FROM t LIMIT 1.25tail"),
        Err(Error::Parse {
            ref message,
            span_start: 22,
            span_end: 30,
        }) if message == "malformed numeric token"
    ));
}

#[test]
fn a_signed_pagination_value_reports_the_operator_character() {
    // `+` is an operator character everywhere, so a signed pagination value is
    // rejected by the same diagnostic as any other stray operator.
    assert!(matches!(
        parse("SELECT * FROM t LIMIT +1"),
        Err(Error::Parse {
            ref message,
            span_start: 22,
            span_end: 23,
        }) if message == "unexpected character '+'"
    ));
}

#[test]
fn earlier_pagination_errors_precede_later_deferred_lexer_errors() {
    for (sql, expected_message, marker) in [
        (
            "SELECT * FROM t LIMIT 18446744073709551616 +",
            "LIMIT is outside the u64 range",
            "18446744073709551616",
        ),
        (
            "SELECT * FROM t LIMIT TRUE +",
            "expected an unsigned integer after LIMIT",
            "TRUE",
        ),
        (
            "SELECT * FROM t OFFSET -1 +",
            "OFFSET requires an unsigned integer",
            "-1",
        ),
        (
            "SELECT * FROM t LIMIT 1 LIMIT 2 +",
            "duplicate LIMIT clause",
            "LIMIT",
        ),
    ] {
        let span_start = sql.rfind(marker).expect("fixture contains error marker");
        assert!(matches!(
            parse(sql),
            Err(Error::Parse {
                ref message,
                span_start: actual_start,
                span_end: actual_end,
            }) if message == expected_message
                && (actual_start, actual_end) == (span_start, span_start + marker.len())
        ));
    }
}

#[test]
fn duplicate_and_out_of_order_tail_clauses_are_parse_errors() {
    for sql in [
        "SELECT * FROM t LIMIT 1 LIMIT 2",
        "SELECT * FROM t LIMIT 1 OFFSET 2 OFFSET 3",
        "SELECT * FROM t OFFSET 1 LIMIT 2",
        "SELECT * FROM t LIMIT 1 ORDER BY id",
        "SELECT * FROM t OFFSET 1 ORDER BY id",
        "SELECT * FROM t OFFSET 1 WHERE id = 1",
        "SELECT * FROM t OFFSET 1,",
    ] {
        assert!(
            matches!(parse(sql), Err(Error::Parse { .. })),
            "expected Parse for {sql:?}"
        );
    }
}
