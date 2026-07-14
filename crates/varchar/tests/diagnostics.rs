#![cfg(not(target_family = "wasm"))]

use varchar::{Database, Error, ErrorCode, Limits, Resource};

fn execution_error(database: &mut Database, sql: &str) -> Error {
    match database.execute(sql) {
        Ok(outcome) => panic!("expected {sql:?} to fail, got {outcome:?}"),
        Err(error) => error,
    }
}

fn assert_sql_span(error: &Error, sql: &str, expected: &str) {
    let start = sql.find(expected).expect("expected fragment is present");
    let span = error.span().expect("SQL diagnostic has a span");

    assert_eq!(span.start(), start);
    assert_eq!(span.end(), start + expected.len());
    assert_eq!(span.len(), expected.len());
    assert_eq!(span.range(), start..start + expected.len());
    assert!(!span.is_empty());
    assert_eq!(&sql[span.range()], expected);
}

fn assert_has_only_code(error: &Error, code: ErrorCode) {
    assert_eq!(error.code(), code);
    assert_eq!(error.span(), None);
    assert_eq!(error.storage_offset(), None);
    assert_eq!(error.resource(), None);
    assert_eq!(error.limit(), None);
}

#[test]
fn parse_and_unsupported_errors_report_utf8_byte_spans() {
    let parse_sql = "SELECT 'café' FROM notes ☃";
    let parse_error = execution_error(&mut Database::new(), parse_sql);
    assert_eq!(parse_error.code(), ErrorCode::SqlParse);
    assert_sql_span(&parse_error, parse_sql, "☃");

    let unsupported_sql = "SELECT * FROM notes WHERE body = 'café' ORDER BY body";
    let unsupported_error = execution_error(&mut Database::new(), unsupported_sql);
    assert_eq!(unsupported_error.code(), ErrorCode::UnsupportedSql);
    assert_sql_span(&unsupported_error, unsupported_sql, "ORDER");

    let api_sql = "DELETE FROM notes";
    let api_error = Database::new()
        .explain_select(api_sql)
        .expect_err("explain_select rejects a valid non-SELECT statement");
    assert_eq!(api_error.code(), ErrorCode::UnsupportedSql);
    assert_sql_span(&api_error, api_sql, api_sql);
}

#[test]
fn semantic_errors_have_stable_categories_without_syntax_metadata() {
    let schema_error = execution_error(&mut Database::new(), "SELECT * FROM missing");
    assert_has_only_code(&schema_error, ErrorCode::Schema);

    let mut typed = Database::new();
    typed
        .execute("CREATE TABLE typed (id INTEGER NOT NULL)")
        .expect("fixture schema is valid");
    let type_error = execution_error(&mut typed, "INSERT INTO typed VALUES ('not an integer')");
    assert_has_only_code(&type_error, ErrorCode::Type);

    let mut keyed = Database::new();
    keyed
        .execute("CREATE TABLE keyed (id INTEGER PRIMARY KEY)")
        .expect("fixture schema is valid");
    keyed
        .execute("INSERT INTO keyed VALUES (1)")
        .expect("fixture row is valid");
    let constraint_error = execution_error(&mut keyed, "INSERT INTO keyed VALUES (1)");
    assert_has_only_code(&constraint_error, ErrorCode::Constraint);
}

#[test]
fn corrupt_storage_reports_an_encoded_blob_byte_offset() {
    let blob = "V2;~S|t|id:I:?|note:T:?;~R|t|I1|Tok%0000zz;";
    let expected_offset = blob.find('%').expect("malformed escape is present");
    let error = Database::from_string(blob.to_owned()).expect_err("storage is malformed");

    assert_eq!(error.code(), ErrorCode::CorruptStorage);
    assert_eq!(error.storage_offset(), Some(expected_offset));
    assert_eq!(error.span(), None);
    assert_eq!(error.resource(), None);
    assert_eq!(error.limit(), None);
}

#[test]
fn configured_limit_errors_pair_the_resource_with_its_limit() {
    let limit = 8;
    let limits = Limits {
        max_sql_bytes: limit,
        ..Limits::default()
    };
    let mut database = Database::with_limits(limits);
    let error = execution_error(&mut database, "SELECT * FROM anything");

    assert_eq!(error.code(), ErrorCode::ResourceLimit);
    assert_eq!(error.resource(), Some(Resource::SqlBytes));
    assert_eq!(error.limit(), Some(limit));
    assert_eq!(error.span(), None);
    assert_eq!(error.storage_offset(), None);
}

#[test]
fn public_diagnostic_identifiers_are_stable() {
    let error_codes = [
        (ErrorCode::SqlParse, "sql_parse"),
        (ErrorCode::UnsupportedSql, "unsupported_sql"),
        (ErrorCode::Schema, "schema"),
        (ErrorCode::Type, "type"),
        (ErrorCode::Constraint, "constraint"),
        (ErrorCode::CorruptStorage, "corrupt_storage"),
        (ErrorCode::RegexCompile, "regex_compile"),
        (ErrorCode::RegexRuntime, "regex_runtime"),
        (ErrorCode::ResourceLimit, "resource_limit"),
        (ErrorCode::Allocation, "allocation"),
        (ErrorCode::Capacity, "capacity"),
    ];
    for (code, stable_name) in error_codes {
        assert_eq!(code.as_str(), stable_name);
    }

    let resources = [
        (Resource::DatabaseBytes, "database_bytes"),
        (Resource::SqlBytes, "sql_bytes"),
        (Resource::WherePredicates, "where_predicates"),
        (Resource::JoinSources, "join_sources"),
        (Resource::GeneratedRegexBytes, "generated_regex_bytes"),
        (Resource::QueryWorkingBytes, "query_working_bytes"),
        (Resource::QueryOutputBytes, "query_output_bytes"),
        (Resource::JoinSteps, "join_steps"),
        (Resource::RegexBacktracking, "regex_backtracking"),
    ];
    for (resource, stable_name) in resources {
        assert_eq!(resource.as_str(), stable_name);
    }
}
