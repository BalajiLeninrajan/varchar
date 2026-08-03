use super::super::parse;
use crate::Error;
use crate::sql::ast::Statement;

#[test]
fn parses_show_tables_and_normalizes_describe_identifiers() {
    assert_eq!(
        parse("sHoW TaBlEs;").expect("SHOW TABLES parses"),
        Statement::ShowTables
    );
}

#[test]
fn metadata_statements_report_exact_missing_argument_spans() {
    assert!(matches!(
        parse("SHOW CREATE"),
        Err(Error::Parse {
            ref message,
            span_start: 5,
            span_end: 11,
        }) if message == "expected keyword TABLES"
    ));
}

#[test]
fn metadata_statement_words_are_reserved_and_cannot_be_used_as_identifiers() {
    for (sql, keyword, marker) in [
        ("CREATE TABLE show (id INTEGER)", "SHOW", "show"),
        ("CREATE TABLE t (tables TEXT)", "TABLES", "tables"),
        ("SELECT id FROM tables", "TABLES", "tables"),
        ("DELETE FROM show", "SHOW", "show"),
    ] {
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

    // The keywords still drive the metadata statements they were reserved for.
    assert_eq!(
        parse("SHOW TABLES").expect("SHOW TABLES parses"),
        Statement::ShowTables
    );
}
