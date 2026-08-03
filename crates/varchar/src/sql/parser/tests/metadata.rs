use super::super::parse;
use crate::Error;
use crate::sql::ast::{DescribeTable, Statement};

#[test]
fn parses_show_tables_and_normalizes_describe_identifiers() {
    assert_eq!(
        parse("sHoW TaBlEs;").expect("SHOW TABLES parses"),
        Statement::ShowTables
    );
    assert_eq!(
        parse("DeScRiBe Accounts").expect("DESCRIBE parses"),
        Statement::DescribeTable(DescribeTable {
            table: String::from("accounts"),
        })
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
    assert!(matches!(
        parse("DESCRIBE"),
        Err(Error::Parse {
            ref message,
            span_start: 8,
            span_end: 8,
        }) if message == "expected an identifier"
    ));
}

#[test]
fn quoted_identifiers_disambiguate_reserved_words() {
    parse("CREATE TABLE \"select\" (\"from\" INTEGER, CHECK (\"from\" >= 0))")
        .expect("quoted reserved identifiers parse");
    assert!(matches!(
        parse("CREATE TABLE \"1x\" (id INTEGER)"),
        Err(Error::Parse {
            ref message,
            span_start: 13,
            span_end: 17,
        }) if message == "quoted identifiers must use the unquoted ASCII identifier grammar"
    ));
}

#[test]
fn metadata_statement_words_are_reserved_and_cannot_be_used_as_identifiers() {
    for (sql, keyword, marker) in [
        ("CREATE TABLE show (id INTEGER)", "SHOW", "show"),
        ("CREATE TABLE t (describe INTEGER)", "DESCRIBE", "describe"),
        ("CREATE TABLE t (tables TEXT)", "TABLES", "tables"),
        ("SELECT describe FROM t", "DESCRIBE", "describe"),
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
    assert_eq!(
        parse("DESCRIBE accounts").expect("DESCRIBE parses"),
        Statement::DescribeTable(DescribeTable {
            table: String::from("accounts"),
        })
    );

    // Quoting them names the tables they would otherwise have been mistaken for.
    assert_eq!(
        parse("DESCRIBE \"show\"").expect("a quoted SHOW is a DESCRIBE argument"),
        Statement::DescribeTable(DescribeTable {
            table: String::from("show"),
        })
    );
}
