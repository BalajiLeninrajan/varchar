use super::*;

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
