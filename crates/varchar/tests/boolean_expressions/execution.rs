use super::*;

#[test]
fn select_update_and_delete_residuals_work_after_reload() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE items (id INTEGER NOT NULL, note TEXT, active BOOLEAN NOT NULL)",
    );
    for sql in [
        "INSERT INTO items VALUES (1, NULL, TRUE)",
        "INSERT INTO items VALUES (2, 'alpha', TRUE)",
        "INSERT INTO items VALUES (3, 'beta', FALSE)",
        "INSERT INTO items VALUES (4, 'keep', TRUE)",
    ] {
        execute(&mut database, sql);
    }

    let mut reloaded =
        Database::from_string(database.into_string()).expect("database reloads before execution");
    assert_eq!(
        rows(
            &mut reloaded,
            "SELECT id FROM items \
             WHERE active = TRUE AND (note IS NULL OR note LIKE 'a%')",
        )
        .into_rows(),
        vec![vec![Value::Integer(1)], vec![Value::Integer(2)]]
    );

    assert_eq!(
        execute(
            &mut reloaded,
            "UPDATE items SET active = FALSE WHERE id = 1 OR id = 4",
        ),
        Outcome::Affected { rows: 2 }
    );
    assert_eq!(
        execute(
            &mut reloaded,
            "DELETE FROM items \
             WHERE (note = 'alpha' AND active = TRUE) \
                OR (note = 'keep' AND active = FALSE)",
        ),
        Outcome::Affected { rows: 2 }
    );

    let mut verified = Database::from_string(reloaded.into_string())
        .expect("database reloads after residual mutations");
    assert_eq!(
        rows(&mut verified, "SELECT id, note, active FROM items").into_rows(),
        vec![
            vec![Value::Integer(1), Value::Null, Value::Boolean(false),],
            vec![
                Value::Integer(3),
                Value::Text(String::from("beta")),
                Value::Boolean(false),
            ],
        ]
    );
}

#[test]
fn pushed_not_equal_and_like_keep_null_semantics() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE values_ (id INTEGER NOT NULL, note TEXT, touched BOOLEAN NOT NULL)",
    );
    for sql in [
        "INSERT INTO values_ VALUES (1, NULL, FALSE)",
        "INSERT INTO values_ VALUES (2, 'alpha', FALSE)",
        "INSERT INTO values_ VALUES (3, 'beta', FALSE)",
    ] {
        execute(&mut database, sql);
    }
    let mut database = Database::from_string(database.into_string()).expect("NULL fixture reloads");

    assert_eq!(
        rows(
            &mut database,
            "SELECT id FROM values_ WHERE note != 'alpha'",
        )
        .into_rows(),
        vec![vec![Value::Integer(3)]]
    );
    assert_eq!(
        rows(&mut database, "SELECT id FROM values_ WHERE note LIKE '%'").into_rows(),
        vec![vec![Value::Integer(2)], vec![Value::Integer(3)]]
    );
    assert_eq!(
        execute(
            &mut database,
            "UPDATE values_ SET touched = TRUE WHERE note != 'alpha'",
        ),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        execute(&mut database, "DELETE FROM values_ WHERE note LIKE 'a%'"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT id, note, touched FROM values_").into_rows(),
        vec![
            vec![Value::Integer(1), Value::Null, Value::Boolean(false),],
            vec![
                Value::Integer(3),
                Value::Text(String::from("beta")),
                Value::Boolean(true),
            ],
        ]
    );
}

#[test]
fn pushed_and_residual_like_matchers_stay_in_parity_after_reload() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE like_parity (id INTEGER NOT NULL, value TEXT)",
    );
    for sql in [
        "INSERT INTO like_parity VALUES (1, NULL)",
        "INSERT INTO like_parity VALUES (2, 'a💾b')",
        "INSERT INTO like_parity VALUES (3, 'aéb')",
        "INSERT INTO like_parity VALUES (4, 'aéb')",
        "INSERT INTO like_parity VALUES (5, 'a%b')",
        "INSERT INTO like_parity VALUES (6, 'a_b')",
        r"INSERT INTO like_parity VALUES (7, 'a\b')",
        "INSERT INTO like_parity VALUES (8, 'ab')",
        "INSERT INTO like_parity VALUES (9, 'aXXb')",
    ] {
        execute(&mut database, sql);
    }
    let mut database =
        Database::from_string(database.into_string()).expect("LIKE parity fixture reloads");

    for pattern in [
        "%", "a_b", "%é%", "%é%", "a%%__b", r"%\%%", r"%\_%", r"%\\%",
    ] {
        let pushed = format!("SELECT id FROM like_parity WHERE value LIKE '{pattern}'");
        let residual =
            format!("SELECT id FROM like_parity WHERE value LIKE '{pattern}' OR id = -1");
        assert_eq!(
            rows(&mut database, &pushed).into_rows(),
            rows(&mut database, &residual).into_rows(),
            "pushed and residual LIKE differ for pattern {pattern:?}"
        );
    }
}

#[test]
fn residual_like_is_charged_against_the_regex_backtracking_budget() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE like_budget (id INTEGER NOT NULL, value TEXT)",
    );
    execute(
        &mut database,
        &format!(
            "INSERT INTO like_budget VALUES (1, '{}')",
            "a".repeat(2_048)
        ),
    );
    let blob = database.into_string();
    // The OR keeps the LIKE out of the scan pattern, so nothing bounds it but
    // the budget that would have applied had it been pushed into the regex. The
    // interior literal run is the shape that is retried at every scalar.
    let select = "SELECT id FROM like_budget WHERE value LIKE '%aaaaaaaaaab%' OR id = -1";
    let update = "UPDATE like_budget SET id = 2 WHERE value LIKE '%aaaaaaaaaab%' OR id = -1";
    let delete = "DELETE FROM like_budget WHERE value LIKE '%aaaaaaaaaab%' OR id = -1";
    let limits = Limits {
        regex_backtrack_limit: 64,
        ..Limits::default()
    };

    for sql in [select, update, delete] {
        let mut limited =
            Database::from_string_with_limits(blob.clone(), limits.clone()).expect("blob reloads");
        assert!(
            matches!(
                limited.execute(sql),
                Err(Error::ResourceLimit {
                    resource: Resource::RegexBacktracking,
                    limit: 64,
                })
            ),
            "residual LIKE escaped its budget in {sql:?}"
        );
        assert_eq!(limited.as_str(), blob);
    }

    // The prefilter itself needs no backtracking, so the same budget accepts an
    // equivalent residual without a LIKE. The refusals above are the matcher's.
    let mut control =
        Database::from_string_with_limits(blob.clone(), limits).expect("blob reloads");
    assert!(
        rows(
            &mut control,
            "SELECT id FROM like_budget WHERE id = 1 OR id = -1"
        )
        .rows()
        .len()
            == 1
    );

    // Under the default budget the residual LIKE simply evaluates.
    let mut unlimited = Database::from_string(blob.clone()).expect("blob reloads");
    assert!(rows(&mut unlimited, select).rows().is_empty());
    assert_eq!(
        rows(
            &mut unlimited,
            "SELECT id FROM like_budget WHERE value LIKE '%aa' OR id = -1",
        )
        .into_rows(),
        vec![vec![Value::Integer(1)]]
    );
    assert_eq!(unlimited.as_str(), blob);
}

#[test]
fn one_backtracking_budget_covers_every_residual_like_in_a_statement() {
    // A budget handed out per predicate would let one statement multiply it by
    // the number of residual LIKE leaves it carries, so a statement that stays
    // inside the budget one predicate at a time could still run unbounded.
    const BUDGET: usize = 4_000;

    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE like_share (id INTEGER NOT NULL, value TEXT)",
    );
    execute(
        &mut database,
        &format!(
            "INSERT INTO like_share VALUES (1, '{}b{}')",
            "a".repeat(300),
            "a".repeat(300)
        ),
    );
    let blob = database.into_string();
    let limits = Limits {
        regex_backtrack_limit: BUDGET,
        ..Limits::default()
    };

    // Every group matches, so none of them short-circuits the ones after it.
    let group = "(id = -1 OR value LIKE '%aaaaaaaaaaaaaaaaab%')";
    let single = format!("SELECT id FROM like_share WHERE {group}");
    let many = format!(
        "SELECT id FROM like_share WHERE {}",
        [group; 8].join(" AND ")
    );

    let mut accepted =
        Database::from_string_with_limits(blob.clone(), limits.clone()).expect("blob reloads");
    assert_eq!(
        rows(&mut accepted, &single).into_rows(),
        vec![vec![Value::Integer(1)]],
        "one residual LIKE must fit inside the shared budget"
    );

    let mut refused =
        Database::from_string_with_limits(blob.clone(), limits).expect("blob reloads");
    assert!(
        matches!(
            refused.execute(&many),
            Err(Error::ResourceLimit {
                resource: Resource::RegexBacktracking,
                limit: BUDGET,
            })
        ),
        "residual LIKE predicates were each given their own budget"
    );
    assert_eq!(refused.as_str(), blob);
}

#[test]
fn long_residual_like_scans_stay_in_parity_with_pushed_down_scans() {
    // A `%`-led pattern is anchored at the end of the value, so it costs the
    // pattern rather than the product of both lengths. The residual and pushed
    // forms must therefore agree at sizes where a rescanning matcher would burn
    // through the default budget and refuse the residual form outright.
    let body = "a".repeat(60_000);
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE like_long (id INTEGER NOT NULL, body TEXT)",
    );
    execute(
        &mut database,
        &format!("INSERT INTO like_long VALUES (1, '{body}')"),
    );
    execute(
        &mut database,
        &format!(
            "INSERT INTO like_long VALUES (2, '{}b')",
            "a".repeat(59_999)
        ),
    );
    let mut database = Database::from_string(database.into_string()).expect("fixture reloads");

    for run in [12_usize, 16, 20, 24] {
        let pattern = format!("%{}b", "a".repeat(run));
        let pushed = format!("SELECT id FROM like_long WHERE body LIKE '{pattern}'");
        let residual = format!("SELECT id FROM like_long WHERE body LIKE '{pattern}' OR id = -1");
        assert_eq!(
            rows(&mut database, &residual).into_rows(),
            rows(&mut database, &pushed).into_rows(),
            "pushed and residual LIKE differ for a {run}-scalar literal run"
        );
    }

    // The pattern the finding was raised against: a 30,000-atom literal run
    // against a 60,000-scalar cell, which no longer rescans the value at all.
    let adversarial = format!(
        "SELECT id FROM like_long WHERE body LIKE '%{}b' OR id = -1",
        "a".repeat(30_000)
    );
    assert_eq!(
        rows(&mut database, &adversarial).into_rows(),
        vec![vec![Value::Integer(2)]]
    );
}

#[test]
fn cross_source_residuals_use_three_valued_truth_without_reordering() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE left_rows (id INTEGER NOT NULL, flag BOOLEAN)",
    );
    execute(
        &mut database,
        "CREATE TABLE right_rows (id INTEGER NOT NULL, flag BOOLEAN)",
    );
    for sql in [
        "INSERT INTO left_rows VALUES (1, NULL)",
        "INSERT INTO left_rows VALUES (2, NULL)",
        "INSERT INTO left_rows VALUES (3, FALSE)",
        "INSERT INTO left_rows VALUES (4, TRUE)",
        "INSERT INTO right_rows VALUES (1, FALSE)",
        "INSERT INTO right_rows VALUES (2, TRUE)",
        "INSERT INTO right_rows VALUES (3, NULL)",
        "INSERT INTO right_rows VALUES (4, NULL)",
    ] {
        execute(&mut database, sql);
    }
    let mut database = Database::from_string(database.into_string()).expect("JOIN fixture reloads");

    assert_eq!(
        rows(
            &mut database,
            "SELECT left_rows.id \
             FROM left_rows JOIN right_rows ON left_rows.id = right_rows.id \
             WHERE left_rows.flag = TRUE OR right_rows.flag = TRUE",
        )
        .into_rows(),
        vec![vec![Value::Integer(2)], vec![Value::Integer(4)]]
    );
}
