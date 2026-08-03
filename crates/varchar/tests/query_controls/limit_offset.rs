use varchar::{Database, Error, Limits, Outcome, Resource, RowSet, Value};

fn execute(database: &mut Database, sql: &str) -> Outcome {
    database
        .execute(sql)
        .unwrap_or_else(|error| panic!("failed to execute {sql:?}: {error}"))
}

fn row_set(database: &mut Database, sql: &str) -> RowSet {
    match execute(database, sql) {
        Outcome::Rows(rows) => rows,
        other => panic!("expected rows for {sql:?}, got {other:?}"),
    }
}

fn rows(database: &mut Database, sql: &str) -> Vec<Vec<Value>> {
    row_set(database, sql).into_rows()
}

fn ids(database: &mut Database, sql: &str) -> Vec<i64> {
    rows(database, sql)
        .into_iter()
        .map(|row| match row.as_slice() {
            [Value::Integer(id)] => *id,
            other => panic!("expected one INTEGER, got {other:?}"),
        })
        .collect()
}

fn pagination_fixture() -> Database {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE events (id INTEGER NOT NULL, priority INTEGER, payload TEXT NOT NULL)",
    );
    for sql in [
        "INSERT INTO events VALUES (1, 20, 'one')",
        "INSERT INTO events VALUES (2, NULL, 'two')",
        "INSERT INTO events VALUES (3, 10, 'three')",
        "INSERT INTO events VALUES (4, 20, 'four')",
        "INSERT INTO events VALUES (5, 30, 'five')",
    ] {
        execute(&mut database, sql);
    }
    database
}

#[test]
fn paginates_unordered_and_ordered_results_after_filtering() {
    let mut database = pagination_fixture();
    let before = database.as_str().to_owned();

    assert_eq!(ids(&mut database, "SELECT id FROM events LIMIT 2"), [1, 2]);
    assert_eq!(
        ids(&mut database, "SELECT id FROM events OFFSET 2"),
        [3, 4, 5]
    );
    assert_eq!(
        ids(&mut database, "SELECT id FROM events LIMIT 2 OFFSET 2"),
        [3, 4]
    );
    assert_eq!(
        ids(
            &mut database,
            "SELECT id FROM events WHERE id != 3 \
             ORDER BY priority DESC, id ASC LIMIT 2 OFFSET 1",
        ),
        [5, 1]
    );
    assert_eq!(
        ids(
            &mut database,
            "SELECT id FROM events LIMIT 18446744073709551615 OFFSET 0001",
        ),
        [2, 3, 4, 5]
    );
    assert_eq!(
        database.as_str(),
        before,
        "pagination never mutates storage"
    );

    let blob = database.into_string();
    let mut reloaded = Database::from_string(blob.clone()).expect("fixture reloads");
    assert_eq!(
        ids(
            &mut reloaded,
            "SELECT id FROM events ORDER BY priority, id LIMIT 3 OFFSET 1",
        ),
        [1, 4, 5]
    );
    assert_eq!(reloaded.as_str(), blob);
}

#[test]
fn huge_offsets_return_normal_metadata_and_no_rows() {
    let mut database = pagination_fixture();

    let result = row_set(
        &mut database,
        "SELECT payload FROM events OFFSET 18446744073709551615",
    );
    assert_eq!(result.columns().len(), 1);
    assert_eq!(result.columns()[0].label(), "payload");
    assert!(result.rows().is_empty());

    let result = row_set(
        &mut database,
        "SELECT payload FROM events ORDER BY id OFFSET 18446744073709551615",
    );
    assert_eq!(result.columns().len(), 1);
    assert_eq!(result.columns()[0].label(), "payload");
    assert!(result.rows().is_empty());
}

#[test]
fn join_offsets_count_qualifying_joined_rows_and_limit_stops_traversal() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE parents (id INTEGER NOT NULL)");
    execute(
        &mut database,
        "CREATE TABLE children (id INTEGER NOT NULL, parent_id INTEGER NOT NULL)",
    );
    execute(&mut database, "INSERT INTO parents VALUES (1)");
    execute(&mut database, "INSERT INTO parents VALUES (2)");
    for sql in [
        "INSERT INTO children VALUES (10, 1)",
        "INSERT INTO children VALUES (11, 2)",
        "INSERT INTO children VALUES (12, 1)",
        "INSERT INTO children VALUES (13, 1)",
    ] {
        execute(&mut database, sql);
    }
    let blob = database.into_string();
    let limits = Limits {
        max_join_steps: 6,
        ..Limits::default()
    };
    let mut limited =
        Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");
    let join = "FROM parents JOIN children ON parents.id = children.parent_id";

    assert_eq!(
        ids(
            &mut limited,
            &format!("SELECT children.id {join} LIMIT 2 OFFSET 2"),
        ),
        [13, 11]
    );
    assert!(matches!(
        limited.execute(&format!("SELECT children.id {join} OFFSET 2")),
        Err(Error::ResourceLimit {
            resource: Resource::JoinSteps,
            limit: 6,
        })
    ));
    assert_eq!(limited.as_str(), blob);
}

#[test]
fn limit_zero_completes_planning_but_skips_all_execution_work() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE left_rows (id INTEGER NOT NULL, name TEXT)",
    );
    execute(
        &mut database,
        "CREATE TABLE right_rows (id INTEGER NOT NULL)",
    );
    execute(&mut database, "INSERT INTO left_rows VALUES (1, 'payload')");
    execute(&mut database, "INSERT INTO right_rows VALUES (1)");
    let blob = database.into_string();
    let limits = Limits {
        max_query_working_bytes: 0,
        max_join_steps: 0,
        ..Limits::default()
    };
    let mut limited =
        Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");

    let result = row_set(
        &mut limited,
        "SELECT left_rows.name FROM left_rows \
         JOIN right_rows ON left_rows.id = right_rows.id \
         WHERE (left_rows.id = 1 OR right_rows.id = 1) \
         ORDER BY right_rows.id LIMIT 0 OFFSET 18446744073709551615",
    );
    assert_eq!(result.columns().len(), 1);
    assert!(result.rows().is_empty());
    assert_eq!(limited.as_str(), blob);
}

#[test]
fn limit_zero_still_reports_semantic_pattern_and_metadata_errors() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE first (id INTEGER NOT NULL, name TEXT)",
    );
    execute(&mut database, "CREATE TABLE second (id INTEGER NOT NULL)");
    let blob = database.as_str().to_owned();

    assert!(matches!(
        database.execute("SELECT missing FROM first LIMIT 0"),
        Err(Error::Schema(_))
    ));
    assert!(matches!(
        database.execute(
            "SELECT first.id FROM first JOIN second ON first.id = second.id \
             ORDER BY id LIMIT 0",
        ),
        Err(Error::Schema(_))
    ));
    assert!(matches!(
        database.execute("SELECT id FROM first WHERE id = 'wrong' LIMIT 0"),
        Err(Error::Type(_))
    ));
    assert!(matches!(
        database.execute("SELECT id FROM first WHERE name LIKE 'bad\\' LIMIT 0"),
        Err(Error::Type(_))
    ));

    let predicate_limits = Limits {
        max_predicates: 1,
        ..Limits::default()
    };
    let mut predicate_limited =
        Database::from_string_with_limits(blob.clone(), predicate_limits).expect("fixture reloads");
    assert!(matches!(
        predicate_limited.execute("SELECT id FROM first WHERE id = 1 OR id = 2 LIMIT 0"),
        Err(Error::ResourceLimit {
            resource: Resource::WherePredicates,
            limit: 1,
        })
    ));

    let pattern_limits = Limits {
        max_pattern_bytes: 1,
        ..Limits::default()
    };
    let mut pattern_limited =
        Database::from_string_with_limits(blob.clone(), pattern_limits).expect("fixture reloads");
    assert!(matches!(
        pattern_limited.execute("SELECT id FROM first LIMIT 0"),
        Err(Error::ResourceLimit {
            resource: Resource::GeneratedRegexBytes,
            limit: 1,
        })
    ));

    let output_limits = Limits {
        max_query_output_bytes: 0,
        ..Limits::default()
    };
    let mut output_limited =
        Database::from_string_with_limits(blob, output_limits).expect("fixture reloads");
    assert!(matches!(
        output_limited.execute("SELECT id FROM first LIMIT 0"),
        Err(Error::ResourceLimit {
            resource: Resource::QueryOutputBytes,
            limit: 0,
        })
    ));
}

#[test]
fn output_budget_charges_only_the_final_paginated_rows() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE payloads (id INTEGER NOT NULL, payload TEXT NOT NULL)",
    );
    let payload = "x".repeat(4_096);
    for id in 1..=3 {
        execute(
            &mut database,
            &format!("INSERT INTO payloads VALUES ({id}, '{payload}')"),
        );
    }
    let blob = database.into_string();

    for sql in [
        "SELECT payload FROM payloads LIMIT 1 OFFSET 1",
        "SELECT payload FROM payloads ORDER BY id DESC LIMIT 1 OFFSET 1",
    ] {
        let exact = minimum_output_limit(&blob, sql);
        let limits = Limits {
            max_query_output_bytes: exact,
            ..Limits::default()
        };
        let mut exact_database =
            Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");
        assert_eq!(rows(&mut exact_database, sql).len(), 1);

        let limits = Limits {
            max_query_output_bytes: exact - 1,
            ..Limits::default()
        };
        let mut one_under =
            Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");
        assert!(matches!(
            one_under.execute(sql),
            Err(Error::ResourceLimit {
                resource: Resource::QueryOutputBytes,
                limit,
            }) if limit == exact - 1
        ));

        let limits = Limits {
            max_query_output_bytes: exact,
            ..Limits::default()
        };
        let mut unpaginated =
            Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");
        assert!(matches!(
            unpaginated.execute("SELECT payload FROM payloads"),
            Err(Error::ResourceLimit {
                resource: Resource::QueryOutputBytes,
                limit,
            }) if limit == exact
        ));
    }
}

#[test]
fn ordered_pagination_retains_only_its_own_window_of_qualifying_rows() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE ordered (id INTEGER NOT NULL, key_ TEXT NOT NULL)",
    );
    for id in 0..8 {
        execute(
            &mut database,
            &format!("INSERT INTO ordered VALUES ({id}, 'payload-{id}')"),
        );
    }
    let blob = database.into_string();
    let unbounded = minimum_working_limit(&blob, "SELECT id FROM ordered ORDER BY key_");

    // Ordered working bytes scale with `OFFSET + LIMIT`, not with the number of
    // qualifying rows, and two windows of the same width cost the same.
    let one = minimum_working_limit(&blob, "SELECT id FROM ordered ORDER BY key_ LIMIT 1");
    let four = minimum_working_limit(&blob, "SELECT id FROM ordered ORDER BY key_ LIMIT 4");
    let offset_three = minimum_working_limit(
        &blob,
        "SELECT id FROM ordered ORDER BY key_ LIMIT 1 OFFSET 3",
    );
    assert!(
        one < four,
        "a one-row window costs less than a four-row one"
    );
    assert_eq!(
        four, offset_three,
        "LIMIT 1 OFFSET 3 retains the same four rows as LIMIT 4"
    );
    assert!(
        four < unbounded,
        "a four-row window costs less than all eight qualifying rows"
    );

    // A window at least as wide as the result cannot save anything, and an
    // open-ended window still has to retain every qualifying row.
    assert_eq!(
        minimum_working_limit(&blob, "SELECT id FROM ordered ORDER BY key_ LIMIT 8"),
        unbounded
    );
    assert_eq!(
        minimum_working_limit(
            &blob,
            "SELECT id FROM ordered ORDER BY key_ OFFSET 18446744073709551615",
        ),
        unbounded
    );

    // Evicted rows refund their working charge, so the window's own bound is
    // enough to produce the window itself.
    let limits = Limits {
        max_query_working_bytes: offset_three,
        ..Limits::default()
    };
    let mut bounded = Database::from_string_with_limits(blob, limits).expect("fixture reloads");
    assert_eq!(
        ids(
            &mut bounded,
            "SELECT id FROM ordered ORDER BY key_ LIMIT 1 OFFSET 3",
        ),
        [3]
    );
}

fn minimum_output_limit(blob: &str, sql: &str) -> usize {
    let mut lower = 0_usize;
    let mut upper = 64 * 1024_usize;
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        let limits = Limits {
            max_query_output_bytes: middle,
            ..Limits::default()
        };
        let mut database =
            Database::from_string_with_limits(blob.to_owned(), limits).expect("fixture reloads");
        match database.execute(sql) {
            Ok(_) => upper = middle,
            Err(Error::ResourceLimit {
                resource: Resource::QueryOutputBytes,
                ..
            }) => lower = middle + 1,
            Err(error) => panic!("unexpected error while finding output boundary: {error}"),
        }
    }
    assert!(lower > 0, "result has a nonzero output charge");
    lower
}

fn minimum_working_limit(blob: &str, sql: &str) -> usize {
    let mut lower = 0_usize;
    let mut upper = 64 * 1024_usize;
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        let limits = Limits {
            max_query_working_bytes: middle,
            ..Limits::default()
        };
        let mut database =
            Database::from_string_with_limits(blob.to_owned(), limits).expect("fixture reloads");
        match database.execute(sql) {
            Ok(_) => upper = middle,
            Err(Error::ResourceLimit {
                resource: Resource::QueryWorkingBytes,
                ..
            }) => lower = middle + 1,
            Err(error) => panic!("unexpected error while finding working boundary: {error}"),
        }
    }
    assert!(lower > 0, "ordered collection has a nonzero working charge");
    lower
}
