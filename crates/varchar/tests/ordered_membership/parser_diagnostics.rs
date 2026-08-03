use super::*;

fn assert_parse_error(
    database: &mut Database,
    sql: &str,
    expected_message: &str,
    span_start: usize,
    span_end: usize,
) {
    let before = database.as_str().to_owned();
    assert!(matches!(
        database.execute(sql),
        Err(Error::Parse {
            ref message,
            span_start: actual_start,
            span_end: actual_end,
        }) if message == expected_message
            && (actual_start, actual_end) == (span_start, span_end)
    ));
    assert_eq!(database.as_str(), before);
}

fn assert_unsupported(
    database: &mut Database,
    sql: &str,
    expected_feature: &str,
    span_start: usize,
    span_end: usize,
) {
    let before = database.as_str().to_owned();
    assert!(matches!(
        database.execute(sql),
        Err(Error::Unsupported {
            ref feature,
            span_start: actual_start,
            span_end: actual_end,
        }) if feature == expected_feature
            && (actual_start, actual_end) == (span_start, span_end)
    ));
    assert_eq!(database.as_str(), before);
}

#[test]
fn malformed_and_excluded_comparison_operators_have_exact_public_diagnostics() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE operator_values (id INTEGER NOT NULL)",
    );

    for operator in ["==", "=>", "<<"] {
        let sql = format!("SELECT id FROM operator_values WHERE id {operator} 1");
        let span_start = sql.find(operator).expect("fixture contains operator");
        assert_parse_error(
            &mut database,
            &sql,
            &format!("malformed comparison operator `{operator}`"),
            span_start,
            span_start + operator.len(),
        );
    }

    let sql = "SELECT id FROM operator_values WHERE id ! 1";
    let span_start = sql.find('!').expect("fixture contains operator");
    assert_parse_error(
        &mut database,
        sql,
        "expected `=` after `!`",
        span_start,
        span_start + 1,
    );

    let sql = "SELECT id FROM operator_values WHERE id <> 1";
    let span_start = sql.find("<>").expect("fixture contains operator");
    assert_unsupported(
        &mut database,
        sql,
        "comparison operator `<>`",
        span_start,
        span_start + 2,
    );
}

#[test]
fn ordered_comparisons_outside_where_keep_parent_diagnostics() {
    let mut database = Database::new();

    for sql in [
        "SELECT a.id FROM a JOIN b ON a.id < b.id",
        "SELECT id > 0 FROM t",
        "SELECT id <> 0 FROM t",
        "INSERT INTO t VALUES (1 <= 2)",
        "UPDATE t SET id = 1 >= 2",
        "CREATE TABLE t (id INTEGER CHECK (id < 2))",
        "SELECT id << 1 FROM t",
    ] {
        let span_start = sql
            .find('<')
            .or_else(|| sql.find('>'))
            .expect("fixture contains ordered comparison");
        assert_unsupported(
            &mut database,
            sql,
            "ordered comparisons",
            span_start,
            span_start + 1,
        );
    }

    let sql = "SELECT id => 1 FROM t";
    let span_start = sql.find('>').expect("fixture contains greater-than sign");
    assert_unsupported(
        &mut database,
        sql,
        "ordered comparisons",
        span_start,
        span_start + 1,
    );

    let sql = "SELECT id !< 1 FROM t";
    let span_start = sql.find('!').expect("fixture contains exclamation mark");
    assert_parse_error(
        &mut database,
        sql,
        "expected `=` after `!`",
        span_start,
        span_start + 1,
    );

    let sql = "SELECT id == 1 FROM t";
    let span_start = sql.find('=').expect("fixture contains equals sign");
    assert_parse_error(
        &mut database,
        sql,
        "expected keyword FROM",
        span_start,
        span_start + 1,
    );
}

#[test]
fn deferred_operators_keep_precedence_over_later_lexical_errors() {
    let mut database = Database::new();

    let sql = "SELECT id < \"x\" FROM t";
    let span_start = sql.find('<').expect("fixture contains less-than sign");
    assert_unsupported(
        &mut database,
        sql,
        "ordered comparisons",
        span_start,
        span_start + 1,
    );
}

#[test]
fn malformed_where_diagnostics_precede_later_valid_ordered_operators() {
    let mut database = Database::new();

    for sql in [
        "SELECT * FROM t WHERE a < 1 OR OR b > 2",
        "UPDATE t SET a = 1 WHERE a < 1 OR OR b > 2",
        "DELETE FROM t WHERE a < 1 OR OR b > 2",
    ] {
        let span_start = sql.find("OR OR").expect("fixture contains duplicate OR") + 3;
        assert_parse_error(
            &mut database,
            sql,
            "reserved keyword `OR` cannot be used as an identifier",
            span_start,
            span_start + 2,
        );
    }
}
