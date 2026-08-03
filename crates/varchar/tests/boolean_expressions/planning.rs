use super::*;

#[test]
fn explain_regex_exposes_the_prefilter_without_residual_factors() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE items (id INTEGER NOT NULL, note TEXT NOT NULL, active BOOLEAN NOT NULL)",
    );
    for sql in [
        "INSERT INTO items VALUES (1, 'residualleft', TRUE)",
        "INSERT INTO items VALUES (2, 'residualright', FALSE)",
        "INSERT INTO items VALUES (3, 'other', TRUE)",
    ] {
        execute(&mut database, sql);
    }
    let sql = "SELECT id FROM items \
               WHERE active = TRUE \
                 AND (note = 'residualleft' OR note = 'residualright')";
    let before = database.as_str().to_owned();

    let explanation = database.explain_select(sql).expect("SELECT explains");
    assert!(explanation.pattern().contains("B1"));
    assert!(!explanation.pattern().contains("residualleft"));
    assert!(!explanation.pattern().contains("residualright"));
    // The OR factor stays residual, so the pattern only prefilters.
    assert!(!explanation.pattern_is_exact());
    // The pattern is exactly the pushed `active = TRUE` factor, which retains a
    // row the full WHERE clause rejects: applying it alone over-selects.
    let pushed_only = database
        .explain_select("SELECT id FROM items WHERE active = TRUE")
        .expect("the pushed factor explains");
    assert_eq!(pushed_only.pattern(), explanation.pattern());
    assert!(pushed_only.pattern_is_exact());
    assert_eq!(
        rows(&mut database, "SELECT id FROM items WHERE active = TRUE").into_rows(),
        vec![vec![Value::Integer(1)], vec![Value::Integer(3)]]
    );
    assert_eq!(
        execute(&mut database, &format!("EXPLAIN REGEX {sql}")),
        Outcome::Explain(explanation)
    );
    assert_eq!(
        rows(&mut database, sql).into_rows(),
        vec![vec![Value::Integer(1)]]
    );
    assert_eq!(database.as_str(), before);
}

#[test]
fn explain_reports_an_exact_pattern_only_without_residual_factors() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE items (id INTEGER NOT NULL, note TEXT NOT NULL, active BOOLEAN NOT NULL)",
    );
    for sql in [
        "INSERT INTO items VALUES (1, 'kept', TRUE)",
        "INSERT INTO items VALUES (2, 'dropped', FALSE)",
    ] {
        execute(&mut database, sql);
    }

    let sql = "SELECT id FROM items WHERE active = TRUE AND note LIKE 'k%'";
    let exact = database
        .explain_select(sql)
        .expect("a pushed WHERE explains");
    assert!(exact.pattern_is_exact());
    assert_eq!(
        rows(&mut database, sql).into_rows(),
        vec![vec![Value::Integer(1)]]
    );

    // A `WHERE`-less SELECT pushes nothing, yet still expresses the whole
    // (empty) WHERE clause.
    let unfiltered = database
        .explain_select("SELECT id FROM items")
        .expect("an unfiltered SELECT explains");
    assert!(unfiltered.pattern_is_exact());

    // Projection is not part of the WHERE clause and never makes it inexact.
    let projected = database
        .explain_select("SELECT note FROM items WHERE active = TRUE")
        .expect("a projected SELECT explains");
    assert!(projected.pattern_is_exact());
}

#[test]
fn residual_evaluator_stack_obeys_the_select_working_budget() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE residual_budget (id INTEGER NOT NULL)",
    );
    let blob = database.into_string();
    let limits = Limits {
        max_query_working_bytes: 0,
        ..Limits::default()
    };
    let mut limited =
        Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");

    assert!(matches!(
        limited.execute("SELECT id FROM residual_budget WHERE id = 1 OR id = 2"),
        Err(Error::ResourceLimit {
            resource: Resource::QueryWorkingBytes,
            limit: 0,
        })
    ));
    assert_eq!(limited.as_str(), blob);
}

#[test]
fn source_local_residual_rejection_precedes_join_row_retention() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE left_rows (join_key INTEGER NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE right_rows (join_key INTEGER NOT NULL, keep BOOLEAN NOT NULL, payload TEXT NOT NULL)",
    );
    execute(&mut database, "INSERT INTO left_rows VALUES (1)");
    let payload = "x".repeat(4_096);
    for _ in 0..12 {
        execute(
            &mut database,
            &format!("INSERT INTO right_rows VALUES (1, FALSE, '{payload}')"),
        );
    }
    let blob = database.into_string();
    let limits = Limits {
        max_query_working_bytes: 8_000,
        ..Limits::default()
    };
    let mut limited =
        Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");

    let selected = limited
        .execute(
            "SELECT left_rows.join_key \
             FROM left_rows JOIN right_rows \
               ON left_rows.join_key = right_rows.join_key \
             WHERE right_rows.keep = TRUE OR right_rows.keep IS NULL",
        )
        .expect("rejected source rows are not retained");
    assert!(matches!(selected, Outcome::Rows(ref rows) if rows.rows().is_empty()));
    assert_eq!(limited.as_str(), blob);
}

#[test]
fn cross_source_residual_runs_after_every_join_condition_charge() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE a (first_key INTEGER NOT NULL, second_key INTEGER NOT NULL, keep BOOLEAN NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE b (first_key INTEGER NOT NULL, second_key INTEGER NOT NULL, keep BOOLEAN NOT NULL)",
    );
    execute(&mut database, "INSERT INTO a VALUES (1, 2, FALSE)");
    execute(&mut database, "INSERT INTO b VALUES (1, 2, FALSE)");
    let blob = database.into_string();
    let sql = "SELECT a.first_key \
               FROM a JOIN b ON a.first_key = b.first_key \
                            AND a.second_key = b.second_key \
               WHERE a.keep = TRUE OR b.keep = TRUE";

    let limits = Limits {
        max_join_steps: 1,
        ..Limits::default()
    };
    let mut limited =
        Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");
    assert!(matches!(
        limited.execute(sql),
        Err(Error::ResourceLimit {
            resource: Resource::JoinSteps,
            limit: 1,
        })
    ));
    assert_eq!(limited.as_str(), blob);

    let limits = Limits {
        max_join_steps: 2,
        ..Limits::default()
    };
    let mut exact = Database::from_string_with_limits(blob.clone(), limits)
        .expect("fixture reloads at the exact JOIN-step limit");
    assert!(rows(&mut exact, sql).rows().is_empty());
    assert_eq!(exact.as_str(), blob);
}
