#![cfg(not(target_family = "wasm"))]

use varchar::{Column, DataType, Database, Error, Limits, Outcome, RowSet, Value};

fn execute(database: &mut Database, sql: &str) -> Outcome {
    database
        .execute(sql)
        .unwrap_or_else(|error| panic!("failed to execute {sql:?}: {error}"))
}

fn rows(database: &mut Database, sql: &str) -> RowSet {
    match execute(database, sql) {
        Outcome::Rows(rows) => rows,
        other => panic!("expected rows for {sql:?}, got {other:?}"),
    }
}

fn column(name: &str, data_type: DataType, nullable: bool) -> Column {
    Column {
        name: name.to_owned(),
        data_type,
        nullable,
    }
}

fn schema_error(database: &mut Database, sql: &str) -> String {
    let before = database.as_str().to_owned();
    let error = database
        .execute(sql)
        .unwrap_err_or_else(|| panic!("unexpectedly accepted {sql:?}"));
    assert_eq!(database.as_str(), before, "failed SELECT changed state");
    match error {
        Error::Schema(message) => message,
        other => panic!("expected schema error for {sql:?}, got {other:?}"),
    }
}

fn unsupported_error(database: &mut Database, sql: &str) {
    let before = database.as_str().to_owned();
    assert!(
        matches!(database.execute(sql), Err(Error::Unsupported { .. })),
        "expected unsupported-feature error for {sql:?}"
    );
    assert_eq!(database.as_str(), before, "failed SELECT changed state");
}

fn parse_error(database: &mut Database, sql: &str) {
    let before = database.as_str().to_owned();
    assert!(
        matches!(database.execute(sql), Err(Error::Parse { .. })),
        "expected parse error for {sql:?}"
    );
    assert_eq!(database.as_str(), before, "failed SELECT changed state");
}

#[test]
fn join_and_inner_join_support_qualified_projection() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE authors (id INTEGER NOT NULL, name TEXT NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE books (id INTEGER NOT NULL, author_id INTEGER, title TEXT NOT NULL)",
    );
    for sql in [
        "INSERT INTO authors VALUES (1, 'Ada')",
        "INSERT INTO authors VALUES (2, 'Grace')",
        "INSERT INTO books VALUES (10, 2, 'Compiler')",
        "INSERT INTO books VALUES (11, 1, 'Notes')",
        "INSERT INTO books VALUES (12, 999, 'Orphan')",
    ] {
        execute(&mut database, sql);
    }

    let expected = RowSet {
        columns: vec![
            column("name", DataType::Text, false),
            column("title", DataType::Text, false),
        ],
        rows: vec![
            vec![
                Value::Text("Ada".to_owned()),
                Value::Text("Notes".to_owned()),
            ],
            vec![
                Value::Text("Grace".to_owned()),
                Value::Text("Compiler".to_owned()),
            ],
        ],
    };

    assert_eq!(
        rows(
            &mut database,
            "SELECT authors.name, books.title FROM authors \
             JOIN books ON authors.id = books.author_id",
        ),
        expected
    );
    assert_eq!(
        rows(
            &mut database,
            "SELECT authors.name, books.title FROM authors \
             INNER JOIN books ON authors.id = books.author_id",
        ),
        expected
    );
}

#[test]
fn inner_and_on_remain_contextual_identifiers() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE inner (on INTEGER NOT NULL, value TEXT NOT NULL)",
    );
    execute(&mut database, "INSERT INTO inner VALUES (1, 'kept')");
    execute(&mut database, "INSERT INTO inner VALUES (2, 'filtered')");

    assert_eq!(
        rows(
            &mut database,
            "SELECT inner.on, value FROM inner WHERE inner.on = 1",
        ),
        RowSet {
            columns: vec![
                column("on", DataType::Integer, false),
                column("value", DataType::Text, false),
            ],
            rows: vec![vec![Value::Integer(1), Value::Text("kept".to_owned()),]],
        }
    );
}

#[test]
fn qualified_update_and_delete_predicates_remain_supported() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, active BOOLEAN NOT NULL)",
    );
    execute(&mut database, "INSERT INTO users VALUES (1, FALSE)");
    execute(&mut database, "INSERT INTO users VALUES (2, FALSE)");

    assert_eq!(
        execute(
            &mut database,
            "UPDATE users SET active = TRUE WHERE users.id = 1",
        ),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        execute(&mut database, "DELETE FROM users WHERE users.id = 2"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT id, active FROM users").rows,
        vec![vec![Value::Integer(1), Value::Boolean(true)]]
    );
}

#[test]
fn malformed_join_forms_are_parse_errors_and_atomic() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE users (id INTEGER NOT NULL)");
    execute(
        &mut database,
        "CREATE TABLE posts (user_id INTEGER NOT NULL)",
    );

    for sql in [
        "SELECT * FROM users JOIN posts",
        "SELECT * FROM users JOIN posts ON users.id != posts.user_id",
        "SELECT * FROM users JOIN posts ON users.id = 1",
    ] {
        parse_error(&mut database, sql);
    }
}

#[test]
fn stars_expand_in_source_then_schema_order() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE customers (id INTEGER NOT NULL, name TEXT NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE invoices (id INTEGER NOT NULL, customer_id INTEGER NOT NULL, paid BOOLEAN)",
    );
    execute(&mut database, "INSERT INTO customers VALUES (7, 'Ada')");
    execute(&mut database, "INSERT INTO invoices VALUES (20, 7, TRUE)");

    assert_eq!(
        rows(
            &mut database,
            "SELECT * FROM customers JOIN invoices \
             ON customers.id = invoices.customer_id",
        ),
        RowSet {
            columns: vec![
                column("id", DataType::Integer, false),
                column("name", DataType::Text, false),
                column("id", DataType::Integer, false),
                column("customer_id", DataType::Integer, false),
                column("paid", DataType::Boolean, true),
            ],
            rows: vec![vec![
                Value::Integer(7),
                Value::Text("Ada".to_owned()),
                Value::Integer(20),
                Value::Integer(7),
                Value::Boolean(true),
            ]],
        }
    );

    assert_eq!(
        rows(
            &mut database,
            "SELECT invoices.*, customers.name, customers.* \
             FROM customers JOIN invoices \
             ON customers.id = invoices.customer_id",
        ),
        RowSet {
            columns: vec![
                column("id", DataType::Integer, false),
                column("customer_id", DataType::Integer, false),
                column("paid", DataType::Boolean, true),
                column("name", DataType::Text, false),
                column("id", DataType::Integer, false),
                column("name", DataType::Text, false),
            ],
            rows: vec![vec![
                Value::Integer(20),
                Value::Integer(7),
                Value::Boolean(true),
                Value::Text("Ada".to_owned()),
                Value::Integer(7),
                Value::Text("Ada".to_owned()),
            ]],
        }
    );
}

#[test]
fn many_to_many_matches_preserve_duplicate_multiplicity_and_storage_order() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE left_rows (join_key INTEGER, marker TEXT NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE right_rows (join_key INTEGER, marker TEXT NOT NULL)",
    );
    for sql in [
        "INSERT INTO left_rows VALUES (1, 'L1')",
        "INSERT INTO left_rows VALUES (2, 'L2')",
        "INSERT INTO left_rows VALUES (1, 'L3')",
        "INSERT INTO right_rows VALUES (1, 'R1')",
        "INSERT INTO right_rows VALUES (1, 'R2')",
        "INSERT INTO right_rows VALUES (2, 'R3')",
        "INSERT INTO right_rows VALUES (1, 'R4')",
    ] {
        execute(&mut database, sql);
    }

    assert_eq!(
        rows(
            &mut database,
            "SELECT left_rows.marker, right_rows.marker \
             FROM left_rows JOIN right_rows \
             ON left_rows.join_key = right_rows.join_key",
        )
        .rows,
        vec![
            vec![Value::Text("L1".to_owned()), Value::Text("R1".to_owned())],
            vec![Value::Text("L1".to_owned()), Value::Text("R2".to_owned())],
            vec![Value::Text("L1".to_owned()), Value::Text("R4".to_owned())],
            vec![Value::Text("L2".to_owned()), Value::Text("R3".to_owned())],
            vec![Value::Text("L3".to_owned()), Value::Text("R1".to_owned())],
            vec![Value::Text("L3".to_owned()), Value::Text("R2".to_owned())],
            vec![Value::Text("L3".to_owned()), Value::Text("R4".to_owned())],
        ]
    );
}

#[test]
fn null_join_keys_never_match() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE nullable_left (join_key INTEGER, marker TEXT NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE nullable_right (join_key INTEGER, marker TEXT NOT NULL)",
    );
    for sql in [
        "INSERT INTO nullable_left VALUES (NULL, 'left null')",
        "INSERT INTO nullable_left VALUES (1, 'left one')",
        "INSERT INTO nullable_right VALUES (NULL, 'right null')",
        "INSERT INTO nullable_right VALUES (1, 'right one')",
    ] {
        execute(&mut database, sql);
    }

    assert_eq!(
        rows(
            &mut database,
            "SELECT nullable_left.marker, nullable_right.marker \
             FROM nullable_left JOIN nullable_right \
             ON nullable_left.join_key = nullable_right.join_key",
        )
        .rows,
        vec![vec![
            Value::Text("left one".to_owned()),
            Value::Text("right one".to_owned()),
        ]]
    );
}

#[test]
fn where_predicates_resolve_against_each_join_source() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE members (id INTEGER NOT NULL, active BOOLEAN NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE memberships (member_id INTEGER NOT NULL, team_id INTEGER NOT NULL, role TEXT)",
    );
    for sql in [
        "INSERT INTO members VALUES (1, TRUE)",
        "INSERT INTO members VALUES (2, FALSE)",
        "INSERT INTO memberships VALUES (1, 10, 'owner')",
        "INSERT INTO memberships VALUES (1, 20, 'viewer')",
        "INSERT INTO memberships VALUES (2, 10, 'owner')",
    ] {
        execute(&mut database, sql);
    }

    assert_eq!(
        rows(
            &mut database,
            "SELECT members.id, memberships.team_id \
             FROM members JOIN memberships \
             ON members.id = memberships.member_id \
             WHERE members.active = TRUE AND memberships.role = 'owner'",
        )
        .rows,
        vec![vec![Value::Integer(1), Value::Integer(10)]]
    );
}

#[test]
fn chained_joins_can_reference_any_prior_source_and_conjoin_conditions() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE people (id INTEGER NOT NULL, name TEXT NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE enrollments (person_id INTEGER NOT NULL, course_id INTEGER NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE courses (id INTEGER NOT NULL, student_id INTEGER NOT NULL, title TEXT NOT NULL)",
    );
    for sql in [
        "INSERT INTO people VALUES (1, 'Ada')",
        "INSERT INTO people VALUES (2, 'Grace')",
        "INSERT INTO enrollments VALUES (1, 20)",
        "INSERT INTO enrollments VALUES (1, 10)",
        "INSERT INTO enrollments VALUES (2, 10)",
        "INSERT INTO courses VALUES (10, 1, 'Ada Compilers')",
        "INSERT INTO courses VALUES (10, 2, 'Grace Compilers')",
        "INSERT INTO courses VALUES (20, 1, 'Databases')",
    ] {
        execute(&mut database, sql);
    }

    assert_eq!(
        rows(
            &mut database,
            "SELECT people.name, courses.title \
             FROM people \
             JOIN enrollments ON people.id = enrollments.person_id \
             JOIN courses ON enrollments.course_id = courses.id \
                          AND people.id = courses.student_id \
                          AND people.id = enrollments.person_id",
        )
        .rows,
        vec![
            vec![
                Value::Text("Ada".to_owned()),
                Value::Text("Databases".to_owned()),
            ],
            vec![
                Value::Text("Ada".to_owned()),
                Value::Text("Ada Compilers".to_owned()),
            ],
            vec![
                Value::Text("Grace".to_owned()),
                Value::Text("Grace Compilers".to_owned()),
            ],
        ]
    );
}

#[test]
fn unqualified_columns_must_resolve_uniquely() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE customers (id INTEGER NOT NULL, name TEXT NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE invoices (id INTEGER NOT NULL, customer_id INTEGER NOT NULL, total INTEGER NOT NULL)",
    );
    execute(&mut database, "INSERT INTO customers VALUES (1, 'Ada')");
    execute(&mut database, "INSERT INTO invoices VALUES (9, 1, 50)");

    assert_eq!(
        rows(
            &mut database,
            "SELECT name, total FROM customers JOIN invoices \
             ON customers.id = customer_id WHERE total = 50",
        )
        .rows,
        vec![vec![Value::Text("Ada".to_owned()), Value::Integer(50)]]
    );

    for sql in [
        "SELECT id FROM customers JOIN invoices ON customers.id = customer_id",
        "SELECT name FROM customers JOIN invoices ON id = customer_id",
    ] {
        let message = schema_error(&mut database, sql);
        assert!(
            message.to_ascii_lowercase().contains("ambiguous"),
            "expected an ambiguity diagnostic for {sql:?}, got {message:?}"
        );
    }
}

#[test]
fn unknown_qualifiers_and_columns_are_schema_errors() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE parents (id INTEGER NOT NULL)");
    execute(
        &mut database,
        "CREATE TABLE children (parent_id INTEGER NOT NULL, label TEXT NOT NULL)",
    );

    for sql in [
        "SELECT missing.label FROM parents JOIN children ON parents.id = children.parent_id",
        "SELECT missing.* FROM parents JOIN children ON parents.id = children.parent_id",
        "SELECT children.missing FROM parents JOIN children ON parents.id = children.parent_id",
        "SELECT missing FROM parents JOIN children ON parents.id = children.parent_id",
        "SELECT * FROM parents JOIN missing ON parents.id = missing.parent_id",
        "SELECT children.label FROM parents JOIN children ON parents.id = missing.parent_id",
        "SELECT children.label FROM parents JOIN children ON parents.id = children.missing",
        "SELECT children.label FROM parents JOIN children ON parents.id = children.parent_id WHERE missing.id = 1",
    ] {
        let message = schema_error(&mut database, sql);
        assert!(
            message.to_ascii_lowercase().contains("unknown"),
            "expected an unknown-name diagnostic for {sql:?}, got {message:?}"
        );
    }
}

#[test]
fn join_columns_must_have_the_same_type() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE numbers (id INTEGER NOT NULL)");
    execute(&mut database, "CREATE TABLE labels (id TEXT NOT NULL)");
    let before = database.as_str().to_owned();

    assert!(matches!(
        database.execute("SELECT * FROM numbers JOIN labels ON numbers.id = labels.id"),
        Err(Error::Type(_))
    ));
    assert_eq!(database.as_str(), before);
}

#[test]
fn aliases_self_joins_and_non_inner_join_types_are_rejected() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE nodes (id INTEGER NOT NULL, parent_id INTEGER)",
    );
    execute(
        &mut database,
        "CREATE TABLE labels (node_id INTEGER NOT NULL, label TEXT NOT NULL)",
    );

    for sql in [
        "SELECT * FROM nodes AS n JOIN labels ON n.id = labels.node_id",
        "SELECT * FROM nodes JOIN labels AS l ON nodes.id = l.node_id",
        "SELECT * FROM nodes JOIN nodes ON nodes.parent_id = nodes.id",
        "SELECT * FROM nodes LEFT JOIN labels ON nodes.id = labels.node_id",
        "SELECT * FROM nodes LEFT OUTER JOIN labels ON nodes.id = labels.node_id",
        "SELECT * FROM nodes RIGHT JOIN labels ON nodes.id = labels.node_id",
        "SELECT * FROM nodes FULL JOIN labels ON nodes.id = labels.node_id",
        "SELECT * FROM nodes CROSS JOIN labels ON nodes.id = labels.node_id",
        "SELECT * FROM nodes NATURAL JOIN labels",
    ] {
        unsupported_error(&mut database, sql);
    }

    for sql in [
        "SELECT nodes.id AS node_id FROM nodes",
        "SELECT * FROM nodes n JOIN labels ON n.id = labels.node_id",
        "SELECT * FROM nodes JOIN labels l ON nodes.id = l.node_id",
    ] {
        let before = database.as_str().to_owned();
        assert!(
            database.execute(sql).is_err(),
            "unexpectedly accepted alias syntax {sql:?}"
        );
        assert_eq!(database.as_str(), before, "failed SELECT changed state");
    }
}

#[test]
fn compile_select_and_explain_share_one_join_plan_without_mutating() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER NOT NULL, name TEXT NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE children (parent_id INTEGER NOT NULL, name TEXT NOT NULL)",
    );
    execute(&mut database, "INSERT INTO parents VALUES (1, 'parent')");
    execute(&mut database, "INSERT INTO children VALUES (1, 'child')");
    let sql = "SELECT parents.name, children.name \
               FROM parents JOIN children ON parents.id = children.parent_id \
               WHERE children.name LIKE 'chi%'";
    let before = database.as_str().to_owned();

    let plan = database.compile_select(sql).expect("JOIN SELECT compiles");
    assert!(!plan.pattern().is_empty());
    assert_eq!(
        plan.columns(),
        vec![
            column("name", DataType::Text, false),
            column("name", DataType::Text, false),
        ]
    );
    assert_eq!(database.as_str(), before);

    assert_eq!(
        execute(&mut database, &format!("EXPLAIN REGEX {sql}")),
        Outcome::Explain(plan)
    );
    assert_eq!(database.as_str(), before);
    assert_eq!(rows(&mut database, sql).rows.len(), 1);
    assert_eq!(database.as_str(), before);
}

#[test]
fn joins_work_after_reloading_the_authoritative_string() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER NOT NULL, name TEXT NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE children (id INTEGER NOT NULL, parent_id INTEGER NOT NULL, name TEXT NOT NULL)",
    );
    execute(&mut database, "INSERT INTO parents VALUES (1, 'parent')");
    execute(
        &mut database,
        "INSERT INTO children VALUES (10, 1, 'child')",
    );
    let blob = database.into_string();
    let mut reloaded = Database::from_string(blob.clone()).expect("database reloads");

    assert_eq!(reloaded.as_str(), blob);
    assert_eq!(
        rows(
            &mut reloaded,
            "SELECT parents.name, children.name \
             FROM parents JOIN children ON parents.id = children.parent_id",
        )
        .rows,
        vec![vec![
            Value::Text("parent".to_owned()),
            Value::Text("child".to_owned()),
        ]]
    );
    assert_eq!(reloaded.as_str(), blob);
}

#[test]
fn join_fanout_obeys_the_result_byte_limit() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE left_rows (join_key INTEGER)");
    execute(&mut database, "CREATE TABLE right_rows (join_key INTEGER)");
    for _ in 0..12 {
        execute(&mut database, "INSERT INTO left_rows VALUES (1)");
        execute(&mut database, "INSERT INTO right_rows VALUES (1)");
    }
    let blob = database.into_string();
    let limits = Limits {
        max_result_bytes: 256,
        ..Limits::default()
    };
    let mut limited =
        Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");

    assert!(matches!(
        limited.execute(
            "SELECT * FROM left_rows JOIN right_rows \
             ON left_rows.join_key = right_rows.join_key"
        ),
        Err(Error::ResourceLimit {
            resource: "result bytes",
            limit: 256
        })
    ));
    assert_eq!(limited.as_str(), blob);
}

#[test]
fn join_source_count_has_its_own_limit() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE a (id INTEGER NOT NULL)");
    execute(&mut database, "CREATE TABLE b (id INTEGER NOT NULL)");
    let blob = database.into_string();
    let limits = Limits {
        max_join_sources: 1,
        ..Limits::default()
    };
    let mut limited =
        Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");

    assert!(matches!(
        limited.execute("SELECT * FROM a JOIN b ON a.id = b.id"),
        Err(Error::ResourceLimit {
            resource: "JOIN sources",
            limit: 1,
        })
    ));
    assert_eq!(limited.as_str(), blob);
}

#[test]
fn nonmatching_join_fanout_obeys_the_combination_limit() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE a (join_key INTEGER NOT NULL)");
    execute(&mut database, "CREATE TABLE b (join_key INTEGER NOT NULL)");
    execute(&mut database, "CREATE TABLE c (join_key INTEGER NOT NULL)");
    for _ in 0..8 {
        execute(&mut database, "INSERT INTO a VALUES (1)");
        execute(&mut database, "INSERT INTO b VALUES (1)");
        execute(&mut database, "INSERT INTO c VALUES (2)");
    }
    let blob = database.into_string();
    let limits = Limits {
        max_join_steps: 100,
        ..Limits::default()
    };
    let mut limited =
        Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");

    assert!(matches!(
        limited.execute(
            "SELECT * FROM a \
             JOIN b ON a.join_key = b.join_key \
             JOIN c ON b.join_key = c.join_key"
        ),
        Err(Error::ResourceLimit {
            resource: "JOIN execution steps",
            limit: 100
        })
    ));
    assert_eq!(limited.as_str(), blob);
}

#[test]
fn each_join_condition_evaluation_consumes_the_work_budget() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE a (join_key INTEGER NOT NULL)");
    execute(&mut database, "CREATE TABLE b (join_key INTEGER NOT NULL)");
    for _ in 0..4 {
        execute(&mut database, "INSERT INTO a VALUES (1)");
        execute(&mut database, "INSERT INTO b VALUES (1)");
    }
    let blob = database.into_string();
    let limits = Limits {
        max_join_steps: 50,
        ..Limits::default()
    };
    let mut limited =
        Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");

    assert!(matches!(
        limited.execute(
            "SELECT * FROM a JOIN b ON a.join_key = b.join_key \
             AND a.join_key = b.join_key AND a.join_key = b.join_key \
             AND a.join_key = b.join_key AND a.join_key = b.join_key"
        ),
        Err(Error::ResourceLimit {
            resource: "JOIN execution steps",
            limit: 50
        })
    ));
    assert_eq!(limited.as_str(), blob);
}

#[test]
fn escaped_text_workspace_obeys_the_result_byte_limit() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE left_rows (join_key INTEGER NOT NULL, payload TEXT NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE right_rows (join_key INTEGER NOT NULL)",
    );
    let payload = "|".repeat(100);
    for _ in 0..16 {
        execute(
            &mut database,
            &format!("INSERT INTO left_rows VALUES (1, '{payload}')"),
        );
    }
    execute(&mut database, "INSERT INTO right_rows VALUES (2)");
    let blob = database.into_string();
    let limits = Limits {
        max_result_bytes: 8_000,
        ..Limits::default()
    };
    let mut limited =
        Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");

    assert!(matches!(
        limited.execute(
            "SELECT left_rows.join_key FROM left_rows JOIN right_rows \
             ON left_rows.join_key = right_rows.join_key"
        ),
        Err(Error::ResourceLimit {
            resource: "result bytes",
            limit: 8_000
        })
    ));
    assert_eq!(limited.as_str(), blob);
}

trait ResultExt<T, E> {
    fn unwrap_err_or_else(self, on_ok: impl FnOnce() -> E) -> E;
}

impl<T, E> ResultExt<T, E> for Result<T, E> {
    fn unwrap_err_or_else(self, on_ok: impl FnOnce() -> E) -> E {
        match self {
            Ok(_) => on_ok(),
            Err(error) => error,
        }
    }
}
