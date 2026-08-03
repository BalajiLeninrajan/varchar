use super::*;

#[test]
fn where_exposes_true_results_for_all_three_valued_input_pairs() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE truth_pairs (id INTEGER NOT NULL, left_value BOOLEAN, right_value BOOLEAN)",
    );
    for sql in [
        "INSERT INTO truth_pairs VALUES (1, FALSE, FALSE)",
        "INSERT INTO truth_pairs VALUES (2, FALSE, TRUE)",
        "INSERT INTO truth_pairs VALUES (3, FALSE, NULL)",
        "INSERT INTO truth_pairs VALUES (4, TRUE, FALSE)",
        "INSERT INTO truth_pairs VALUES (5, TRUE, TRUE)",
        "INSERT INTO truth_pairs VALUES (6, TRUE, NULL)",
        "INSERT INTO truth_pairs VALUES (7, NULL, FALSE)",
        "INSERT INTO truth_pairs VALUES (8, NULL, TRUE)",
        "INSERT INTO truth_pairs VALUES (9, NULL, NULL)",
    ] {
        execute(&mut database, sql);
    }

    assert_eq!(
        rows(
            &mut database,
            "SELECT id FROM truth_pairs \
             WHERE left_value = TRUE AND right_value = TRUE",
        )
        .into_rows(),
        vec![vec![Value::Integer(5)]]
    );
    assert_eq!(
        rows(
            &mut database,
            "SELECT id FROM truth_pairs \
             WHERE left_value = TRUE OR right_value = TRUE",
        )
        .into_rows(),
        vec![
            vec![Value::Integer(2)],
            vec![Value::Integer(4)],
            vec![Value::Integer(5)],
            vec![Value::Integer(6)],
            vec![Value::Integer(8)],
        ]
    );
}

#[test]
fn precedence_and_nested_parentheses_have_public_behavior() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE precedence_cases (\
             id INTEGER NOT NULL, \
             a BOOLEAN NOT NULL, \
             b BOOLEAN NOT NULL, \
             c BOOLEAN NOT NULL\
         )",
    );
    for sql in [
        "INSERT INTO precedence_cases VALUES (1, TRUE, FALSE, FALSE)",
        "INSERT INTO precedence_cases VALUES (2, FALSE, TRUE, TRUE)",
        "INSERT INTO precedence_cases VALUES (3, FALSE, TRUE, FALSE)",
        "INSERT INTO precedence_cases VALUES (4, TRUE, FALSE, TRUE)",
    ] {
        execute(&mut database, sql);
    }

    assert_eq!(
        rows(
            &mut database,
            "SELECT id FROM precedence_cases \
             WHERE a = TRUE OR b = TRUE AND c = TRUE",
        )
        .into_rows(),
        vec![
            vec![Value::Integer(1)],
            vec![Value::Integer(2)],
            vec![Value::Integer(4)],
        ]
    );
    assert_eq!(
        rows(
            &mut database,
            "SELECT id FROM precedence_cases \
             WHERE (a = TRUE OR b = TRUE) AND c = TRUE",
        )
        .into_rows(),
        vec![vec![Value::Integer(2)], vec![Value::Integer(4)]]
    );
    assert_eq!(
        rows(
            &mut database,
            "SELECT id FROM precedence_cases \
             WHERE (((a = TRUE OR (b = TRUE AND c = TRUE))))",
        )
        .into_rows(),
        vec![
            vec![Value::Integer(1)],
            vec![Value::Integer(2)],
            vec![Value::Integer(4)],
        ]
    );
}

#[test]
fn excluded_expression_forms_keep_structured_public_errors() {
    let mut database = Database::new();
    let before = database.as_str().to_owned();
    for (sql, expected_feature, marker) in [
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
        let expected_start = sql.find(marker).expect("fixture contains error marker");
        let expected_end = expected_start + marker.len();
        match database.execute(sql) {
            Err(Error::Unsupported {
                feature,
                span_start,
                span_end,
            }) => {
                assert_eq!(feature, expected_feature, "feature for {sql:?}");
                assert_eq!(
                    (span_start, span_end),
                    (expected_start, expected_end),
                    "span for {sql:?}"
                );
            }
            other => panic!("expected exact Unsupported error for {sql:?}, got {other:?}"),
        }
        assert_eq!(database.as_str(), before);
    }
}

#[test]
fn semantically_invalid_short_circuited_branches_fail_before_execution() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE guarded (id INTEGER NOT NULL, note TEXT)",
    );
    execute(&mut database, "INSERT INTO guarded VALUES (1, 'valid')");
    let before = database.as_str().to_owned();

    assert!(matches!(
        database.execute(r"SELECT id FROM guarded WHERE id = 1 OR note LIKE 'bad\q'"),
        Err(Error::Type(ref message))
            if message == "LIKE pattern contains unsupported escape \\q"
    ));
    assert_eq!(database.as_str(), before);

    assert!(matches!(
        database.execute("SELECT id FROM guarded WHERE id = 0 AND missing = 1"),
        Err(Error::Schema(ref message))
            if message == "unknown column \"missing\" in table \"guarded\""
    ));
    assert_eq!(database.as_str(), before);
}

#[test]
fn mutation_residuals_validate_every_branch_after_reload() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE guarded_mutations (id INTEGER NOT NULL, note TEXT, touched BOOLEAN NOT NULL)",
    );
    execute(
        &mut database,
        "INSERT INTO guarded_mutations VALUES (1, 'valid', FALSE)",
    );
    let blob = database.into_string();

    let mut update = Database::from_string(blob.clone()).expect("UPDATE fixture reloads");
    assert!(matches!(
        update.execute(
            "UPDATE guarded_mutations SET touched = TRUE WHERE id = 1 OR missing = 1"
        ),
        Err(Error::Schema(ref message))
            if message == "unknown column \"missing\" in table \"guarded_mutations\""
    ));
    assert_eq!(update.as_str(), blob);

    let mut delete = Database::from_string(blob.clone()).expect("DELETE fixture reloads");
    assert!(matches!(
        delete.execute(
            r"DELETE FROM guarded_mutations WHERE id = 1 OR note LIKE 'invalid\q'"
        ),
        Err(Error::Type(ref message))
            if message == "LIKE pattern contains unsupported escape \\q"
    ));
    assert_eq!(delete.as_str(), blob);
}

#[test]
fn update_and_delete_predicate_limits_accept_exact_and_reject_one_over() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE bounded_mutations (id INTEGER NOT NULL, touched BOOLEAN NOT NULL)",
    );
    execute(
        &mut database,
        "INSERT INTO bounded_mutations VALUES (1, FALSE)",
    );
    execute(
        &mut database,
        "INSERT INTO bounded_mutations VALUES (2, FALSE)",
    );
    let blob = database.into_string();
    let update_sql = "UPDATE bounded_mutations SET touched = TRUE WHERE id = 1 OR id = 2";
    let delete_sql = "DELETE FROM bounded_mutations WHERE id = 1 OR id = 2";

    for (sql, expected_rows) in [(update_sql, 2), (delete_sql, 2)] {
        let limits = Limits {
            max_predicates: 2,
            ..Limits::default()
        };
        let mut exact = Database::from_string_with_limits(blob.clone(), limits)
            .expect("fixture reloads at the exact mutation predicate limit");
        assert_eq!(
            exact.execute(sql).expect("exact predicate count executes"),
            Outcome::Affected {
                rows: expected_rows,
            }
        );
    }

    for sql in [update_sql, delete_sql] {
        let limits = Limits {
            max_predicates: 1,
            ..Limits::default()
        };
        let mut one_over = Database::from_string_with_limits(blob.clone(), limits)
            .expect("fixture reloads below the mutation predicate count");
        assert!(matches!(
            one_over.execute(sql),
            Err(Error::ResourceLimit {
                resource: Resource::WherePredicates,
                limit: 1,
            })
        ));
        assert_eq!(one_over.as_str(), blob);
    }
}

#[test]
fn predicate_limit_accepts_the_exact_count_and_rejects_one_over() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE bounded (id INTEGER NOT NULL, flag BOOLEAN NOT NULL)",
    );
    execute(&mut database, "INSERT INTO bounded VALUES (1, TRUE)");
    execute(&mut database, "INSERT INTO bounded VALUES (2, FALSE)");
    let blob = database.into_string();
    let sql = "SELECT id FROM bounded \
               WHERE (id = 1 OR id = 2) AND (flag = TRUE OR flag = FALSE)";

    let exact_limits = Limits {
        max_predicates: 4,
        ..Limits::default()
    };
    let mut exact = Database::from_string_with_limits(blob.clone(), exact_limits)
        .expect("fixture reloads at the exact predicate limit");
    assert_eq!(
        rows(&mut exact, sql).into_rows(),
        vec![vec![Value::Integer(1)], vec![Value::Integer(2)]]
    );

    let one_over_limits = Limits {
        max_predicates: 3,
        ..Limits::default()
    };
    let mut one_over = Database::from_string_with_limits(blob.clone(), one_over_limits)
        .expect("fixture reloads below the expression predicate count");
    assert!(matches!(
        one_over.execute(sql),
        Err(Error::ResourceLimit {
            resource: Resource::WherePredicates,
            limit: 3,
        })
    ));
    assert_eq!(one_over.as_str(), blob);
}

#[test]
fn public_execution_handles_a_practical_deep_expression() {
    const DEPTH: usize = 2_000;
    let limits = Limits {
        max_predicates: DEPTH + 1,
        ..Limits::default()
    };
    let mut database = Database::with_limits(limits);
    execute(
        &mut database,
        "CREATE TABLE deep_values (value INTEGER NOT NULL)",
    );
    execute(&mut database, "INSERT INTO deep_values VALUES (1)");

    let mut sql = String::from("SELECT value FROM deep_values WHERE ");
    sql.push_str(&"(".repeat(DEPTH));
    sql.push_str("value = 1");
    for index in 0..DEPTH {
        if index % 2 == 0 {
            sql.push_str(" AND value = 1)");
        } else {
            sql.push_str(" OR value = 1)");
        }
    }

    assert_eq!(
        rows(&mut database, &sql).into_rows(),
        vec![vec![Value::Integer(1)]]
    );
}
