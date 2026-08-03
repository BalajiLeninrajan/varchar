use super::*;

#[test]
fn select_update_and_delete_use_ordered_membership_residuals_after_reload() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE jobs (\
             id INTEGER NOT NULL, \
             priority INTEGER, \
             state TEXT, \
             enabled BOOLEAN NOT NULL\
         )",
    );
    for sql in [
        "INSERT INTO jobs VALUES (1, 5, 'queued', TRUE)",
        "INSERT INTO jobs VALUES (2, 10, 'running', TRUE)",
        "INSERT INTO jobs VALUES (3, 20, 'done', TRUE)",
        "INSERT INTO jobs VALUES (4, 30, 'blocked', FALSE)",
        "INSERT INTO jobs VALUES (5, NULL, NULL, TRUE)",
    ] {
        execute(&mut database, sql);
    }

    let mut reloaded =
        Database::from_string(database.into_string()).expect("predicate fixture reloads");
    assert_eq!(
        rows(
            &mut reloaded,
            "SELECT id FROM jobs \
             WHERE (priority >= 10 AND state IN ('queued', 'running', NULL)) \
                OR enabled IN (FALSE)",
        )
        .into_rows(),
        vec![vec![Value::Integer(2)], vec![Value::Integer(4)]]
    );

    assert_eq!(
        execute(
            &mut reloaded,
            "UPDATE jobs SET enabled = FALSE \
             WHERE id IN (1, 3) OR priority > 25",
        ),
        Outcome::Affected { rows: 3 }
    );
    assert_eq!(
        execute(
            &mut reloaded,
            "DELETE FROM jobs \
             WHERE (priority <= 5 AND state IN ('queued', NULL)) \
                OR enabled IN (FALSE)",
        ),
        Outcome::Affected { rows: 3 }
    );

    let mut verified = Database::from_string(reloaded.into_string())
        .expect("predicate mutations reload atomically");
    assert_eq!(
        rows(
            &mut verified,
            "SELECT id, priority, state, enabled FROM jobs"
        )
        .into_rows(),
        vec![
            vec![
                Value::Integer(2),
                Value::Integer(10),
                Value::Text(String::from("running")),
                Value::Boolean(true),
            ],
            vec![
                Value::Integer(5),
                Value::Null,
                Value::Null,
                Value::Boolean(true),
            ],
        ]
    );
}

#[test]
fn ordered_and_membership_predicates_work_in_joined_select_residuals() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE queues (id INTEGER NOT NULL, minimum_priority INTEGER NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE queued_jobs (queue_id INTEGER NOT NULL, priority INTEGER NOT NULL, state TEXT NOT NULL)",
    );
    execute(&mut database, "INSERT INTO queues VALUES (1, 10)");
    execute(&mut database, "INSERT INTO queues VALUES (2, 20)");
    execute(
        &mut database,
        "INSERT INTO queued_jobs VALUES (1, 15, 'queued')",
    );
    execute(
        &mut database,
        "INSERT INTO queued_jobs VALUES (2, 25, 'blocked')",
    );

    let mut database = Database::from_string(database.into_string()).expect("JOIN fixture reloads");
    assert_eq!(
        rows(
            &mut database,
            "SELECT queued_jobs.priority \
             FROM queues JOIN queued_jobs ON queues.id = queued_jobs.queue_id \
             WHERE (queues.minimum_priority >= 20 OR queued_jobs.priority < 20) \
               AND queued_jobs.state IN ('queued', NULL)",
        )
        .into_rows(),
        vec![vec![Value::Integer(15)]]
    );
}

#[test]
fn explain_regex_exposes_the_unfiltered_scan_for_decoded_residuals() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE explained (id INTEGER NOT NULL, state TEXT NOT NULL)",
    );

    let unfiltered = database
        .explain_select("SELECT id FROM explained")
        .expect("unfiltered SELECT explains");
    let residual = database
        .explain_select("SELECT id FROM explained WHERE (id >= 10 OR state IN ('queued', NULL))")
        .expect("residual SELECT explains");
    assert_eq!(residual.pattern(), unfiltered.pattern());
    assert_eq!(
        execute(
            &mut database,
            "EXPLAIN REGEX SELECT id FROM explained \
             WHERE (id >= 10 OR state IN ('queued', NULL))",
        ),
        Outcome::Explain(residual)
    );
}
