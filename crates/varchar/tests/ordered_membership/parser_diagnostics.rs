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
fn arithmetic_operators_remain_lexical_errors_outside_in_lists() {
    let mut database = Database::new();

    for (sql, operator) in [
        ("SELECT id + 1 FROM t", '+'),
        ("SELECT id - 1 FROM t", '-'),
        ("SELECT id / 1 FROM t", '/'),
        ("SELECT id % 1 FROM t", '%'),
    ] {
        let span_start = sql
            .find(operator)
            .expect("fixture contains arithmetic operator");
        assert_parse_error(
            &mut database,
            sql,
            &format!("unexpected character {operator:?}"),
            span_start,
            span_start + 1,
        );
    }
}

#[test]
fn deferred_operators_keep_precedence_over_later_lexical_errors() {
    let mut database = Database::new();

    let sql = "SELECT id + \"x\" FROM t";
    let span_start = sql.find('+').expect("fixture contains plus sign");
    assert_parse_error(
        &mut database,
        sql,
        "unexpected character '+'",
        span_start,
        span_start + 1,
    );

    let sql = "SELECT id < \"x\" FROM t";
    let span_start = sql.find('<').expect("fixture contains less-than sign");
    assert_unsupported(
        &mut database,
        sql,
        "ordered comparisons",
        span_start,
        span_start + 1,
    );

    let sql = "SELECT id FROM t WHERE id IN (1 + \"x\")";
    let span_start = sql.find('+').expect("fixture contains plus sign");
    assert_unsupported(
        &mut database,
        sql,
        "expressions in IN lists",
        span_start,
        span_start + 1,
    );
}

#[test]
fn recognized_in_list_expression_starters_precede_later_lexical_errors() {
    let mut database = Database::new();

    for (tail, marker) in [
        ("1 = @", "="),
        ("1 != @", "!="),
        ("1 < @", "<"),
        ("1 <= @", "<"),
        ("1 > @", ">"),
        ("1 >= @", ">"),
        ("1 * @", "*"),
        ("1 + @", "+"),
        ("1 - @", "-"),
        ("1 / @", "/"),
        ("1 % @", "%"),
        ("1 | @", "|"),
        ("1-2 @", "-"),
        ("1 -2 @", "-"),
        ("1 AND @", "AND"),
        ("1 OR @", "OR"),
        ("1 IS @", "IS"),
        ("1 LIKE @", "LIKE"),
        ("1 IN @", "IN"),
        ("1 BETWEEN @", "BETWEEN"),
        ("1 NOT @", "NOT"),
        ("1 COLLATE @", "COLLATE"),
        ("1, other @", "other"),
        ("1, (2 @", "("),
    ] {
        let sql = format!("SELECT id FROM t WHERE id IN ({tail})");
        let span_start = sql
            .rfind(marker)
            .expect("fixture contains expression starter");
        assert_unsupported(
            &mut database,
            &sql,
            "expressions in IN lists",
            span_start,
            span_start + marker.len(),
        );
    }

    let sql = "SELECT id FROM t WHERE id IN (SELECT @)";
    let span_start = sql.find("SELECT @").expect("fixture contains subquery");
    assert_unsupported(
        &mut database,
        sql,
        "subqueries in IN lists",
        span_start,
        span_start + "SELECT".len(),
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

#[test]
fn in_list_errors_distinguish_excluded_features_from_malformed_syntax() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE list_values (id INTEGER NOT NULL)",
    );
    execute(&mut database, "INSERT INTO list_values VALUES (-2)");
    assert_eq!(
        rows(
            &mut database,
            "SELECT id FROM list_values WHERE id IN (1,-2)"
        )
        .into_rows(),
        vec![vec![Value::Integer(-2)]]
    );

    let sql = "SELECT id FROM list_values WHERE id IN ()";
    let span_start = sql.rfind("IN").expect("fixture contains IN");
    assert_unsupported(
        &mut database,
        sql,
        "empty IN lists",
        span_start,
        span_start + 2,
    );

    let sql = "SELECT id FROM list_values WHERE id IN (SELECT id FROM list_values)";
    let span_start = sql.rfind("SELECT").expect("fixture contains subquery");
    assert_unsupported(
        &mut database,
        sql,
        "subqueries in IN lists",
        span_start,
        span_start + "SELECT".len(),
    );

    let sql = "SELECT id FROM list_values WHERE id IN (1 = 1)";
    let span_start = sql.find('=').expect("fixture contains expression operator");
    assert_unsupported(
        &mut database,
        sql,
        "expressions in IN lists",
        span_start,
        span_start + 1,
    );

    for (sql, marker) in [
        (
            "SELECT id FROM list_values WHERE id IN (1 BETWEEN 0 AND 2)",
            "BETWEEN",
        ),
        ("SELECT id FROM list_values WHERE id IN (1 + 2)", "+"),
        ("SELECT id FROM list_values WHERE id IN (1 - 2)", "-"),
        ("SELECT id FROM list_values WHERE id IN (1-2)", "-"),
        ("SELECT id FROM list_values WHERE id IN (1 / 2)", "/"),
        ("SELECT id FROM list_values WHERE id IN (1 % 2)", "%"),
    ] {
        let span_start = sql
            .find(marker)
            .expect("fixture contains expression marker");
        assert_unsupported(
            &mut database,
            sql,
            "expressions in IN lists",
            span_start,
            span_start + marker.len(),
        );
    }

    let sql = "SELECT id FROM list_values WHERE id IN (1, )";
    let span_start = sql.find(')').expect("fixture contains closing parenthesis");
    assert_parse_error(
        &mut database,
        sql,
        "expected a literal value",
        span_start,
        span_start + 1,
    );
}
