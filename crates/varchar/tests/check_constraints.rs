#![cfg(not(target_family = "wasm"))]

use varchar::{Database, Error, Limits, Outcome, Resource, Value};

fn execute(database: &mut Database, sql: &str) -> Outcome {
    database
        .execute(sql)
        .unwrap_or_else(|error| panic!("failed to execute {sql:?}: {error}"))
}

fn rows(database: &mut Database, sql: &str) -> Vec<Vec<Value>> {
    match execute(database, sql) {
        Outcome::Rows(rows) => rows.into_rows(),
        other => panic!("expected rows for {sql:?}, got {other:?}"),
    }
}

fn atomic_error(database: &mut Database, sql: &str) -> Error {
    let before = database.as_str().to_owned();
    let error = match database.execute(sql) {
        Ok(outcome) => panic!("unexpectedly accepted {sql:?}: {outcome:?}"),
        Err(error) => error,
    };
    assert_eq!(database.as_str(), before, "failed mutation changed state");
    error
}

#[test]
fn check_uses_three_valued_truth_and_all_supported_predicates() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE values_table (number INTEGER CHECK (number >= 0 AND number < 10), name TEXT CHECK (name LIKE 'a_%' OR name IN ('kept', NULL)), flag BOOLEAN CHECK (flag > FALSE OR flag IS NULL))",
    );

    for sql in [
        "INSERT INTO values_table VALUES (0, 'abc', TRUE)",
        "INSERT INTO values_table VALUES (9, 'kept', NULL)",
        "INSERT INTO values_table VALUES (NULL, NULL, NULL)",
        // No IN member matches, but the NULL member makes the result UNKNOWN.
        "INSERT INTO values_table VALUES (5, 'other', TRUE)",
    ] {
        execute(&mut database, sql);
    }

    for sql in [
        "INSERT INTO values_table VALUES (-1, 'abc', TRUE)",
        "INSERT INTO values_table VALUES (10, 'abc', TRUE)",
        "INSERT INTO values_table VALUES (1, 'abc', FALSE)",
    ] {
        assert!(matches!(
            atomic_error(&mut database, sql),
            Error::Constraint(_)
        ));
    }

    assert_eq!(
        rows(&mut database, "SELECT number FROM values_table").len(),
        4
    );

    let mut null_membership = Database::new();
    execute(
        &mut null_membership,
        "CREATE TABLE nullable_membership (value INTEGER CHECK (value IN (NULL)))",
    );
    execute(
        &mut null_membership,
        "INSERT INTO nullable_membership VALUES (1)",
    );
    execute(
        &mut null_membership,
        "INSERT INTO nullable_membership VALUES (NULL)",
    );
}

#[test]
fn equality_and_scalar_ordering_are_type_specific() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE comparisons (\
         exact INTEGER CHECK (exact = 2), \
         different INTEGER CHECK (different != 2), \
         ceiling INTEGER CHECK (ceiling <= 2), \
         text_value TEXT CHECK (text_value > 'A' AND text_value < 'z'), \
         flag BOOLEAN CHECK (flag <= FALSE), \
         required TEXT CHECK (required IS NOT NULL))",
    );

    execute(
        &mut database,
        "INSERT INTO comparisons VALUES (2, 1, 2, 'alpha', FALSE, 'set')",
    );
    execute(
        &mut database,
        "INSERT INTO comparisons VALUES (NULL, NULL, NULL, NULL, NULL, 'set')",
    );

    for sql in [
        "INSERT INTO comparisons VALUES (1, 1, 2, 'alpha', FALSE, 'set')",
        "INSERT INTO comparisons VALUES (2, 2, 2, 'alpha', FALSE, 'set')",
        "INSERT INTO comparisons VALUES (2, 1, 3, 'alpha', FALSE, 'set')",
        "INSERT INTO comparisons VALUES (2, 1, 2, 'A', FALSE, 'set')",
        "INSERT INTO comparisons VALUES (2, 1, 2, 'z', FALSE, 'set')",
        "INSERT INTO comparisons VALUES (2, 1, 2, 'alpha', TRUE, 'set')",
        "INSERT INTO comparisons VALUES (2, 1, 2, 'alpha', FALSE, NULL)",
    ] {
        assert!(matches!(
            atomic_error(&mut database, sql),
            Error::Constraint(_)
        ));
    }
}

#[test]
fn inline_checks_can_reference_later_columns_and_qualified_names_are_rejected() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE later_columns (first INTEGER CHECK (second >= 0), second INTEGER)",
    );
    execute(&mut database, "INSERT INTO later_columns VALUES (1, 0)");
    assert!(matches!(
        atomic_error(&mut database, "INSERT INTO later_columns VALUES (1, -1)"),
        Error::Constraint(ref message)
            if message == "CHECK constraint 1 failed for table \"later_columns\""
    ));

    let mut qualified = Database::new();
    assert!(matches!(
        atomic_error(
            &mut qualified,
            "CREATE TABLE qualified (value INTEGER, CHECK (qualified.value > 0))",
        ),
        Error::Schema(ref message)
            if message
                == "CHECK references must be unqualified local columns; \"value\" is qualified"
    ));
    assert_eq!(qualified.as_str(), "V2;");
}

#[test]
fn declaration_order_and_duplicate_checks_are_preserved() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE ordered_checks (value INTEGER CHECK (value > 0), CHECK (value < 10), CHECK (value > 0))",
    );
    assert_eq!(
        database.as_str(),
        "V3;~S|ordered_checks|value:I:?;~C|ordered_checks|GT|0|I0;~C|ordered_checks|LT|0|I10;~C|ordered_checks|GT|0|I0;"
    );

    assert!(matches!(
        atomic_error(&mut database, "INSERT INTO ordered_checks VALUES (-1)"),
        Error::Constraint(ref message)
            if message == "CHECK constraint 1 failed for table \"ordered_checks\""
    ));
    assert!(matches!(
        atomic_error(&mut database, "INSERT INTO ordered_checks VALUES (10)"),
        Error::Constraint(ref message)
            if message == "CHECK constraint 2 failed for table \"ordered_checks\""
    ));
}

#[test]
fn defaults_and_generated_values_are_applied_before_check() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE jobs (id INTEGER PRIMARY KEY AUTOINCREMENT CHECK (id > 0), state TEXT DEFAULT 'queued' CHECK (state = 'queued'))",
    );

    assert!(matches!(
        atomic_error(
            &mut database,
            "INSERT INTO jobs (state) VALUES ('rejected')",
        ),
        Error::Constraint(ref message)
            if message == "CHECK constraint 2 failed for table \"jobs\""
    ));
    execute(&mut database, "INSERT INTO jobs (state) VALUES ('queued')");
    execute(&mut database, "INSERT INTO jobs (id) VALUES (NULL)");
    assert_eq!(
        rows(&mut database, "SELECT id, state FROM jobs"),
        vec![
            vec![Value::Integer(1), Value::Text("queued".to_owned())],
            vec![Value::Integer(2), Value::Text("queued".to_owned())],
        ]
    );
}

#[test]
fn failed_updates_are_byte_exact_and_checks_survive_reload() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE counters (id INTEGER PRIMARY KEY, value INTEGER CHECK (value >= 0))",
    );
    execute(&mut database, "INSERT INTO counters VALUES (1, 1)");
    execute(&mut database, "INSERT INTO counters VALUES (2, 2)");

    assert!(matches!(
        atomic_error(&mut database, "UPDATE counters SET value = -1"),
        Error::Constraint(_)
    ));
    assert_eq!(
        rows(&mut database, "SELECT id, value FROM counters"),
        vec![
            vec![Value::Integer(1), Value::Integer(1)],
            vec![Value::Integer(2), Value::Integer(2)],
        ]
    );

    let blob = database.into_string();
    let mut reloaded = Database::from_string(blob.clone()).expect("CHECK metadata reloads");
    assert_eq!(reloaded.as_str(), blob);
    assert!(matches!(
        atomic_error(&mut reloaded, "UPDATE counters SET value = -1 WHERE id = 2"),
        Error::Constraint(_)
    ));
}

#[test]
fn check_like_work_limits_mutations_and_persisted_reload_atomically() {
    // An interior literal run is retried at every candidate start, which is the
    // shape whose work outgrows a forward scan and draws on the budget.
    let wide = format!("{}b{}", "a".repeat(60), "a".repeat(3));
    let declaration = format!(
        "CREATE TABLE patterns (value TEXT CHECK (value LIKE '%{}b%' OR value = 'ok'))",
        "a".repeat(20)
    );
    let limits = Limits {
        regex_backtrack_limit: 10,
        ..Limits::default()
    };
    let mut database = Database::with_limits(limits.clone());
    execute(&mut database, &declaration);

    assert!(matches!(
        atomic_error(
            &mut database,
            &format!("INSERT INTO patterns VALUES ('{wide}')")
        ),
        Error::ResourceLimit {
            resource: Resource::RegexBacktracking,
            limit: 10,
        }
    ));

    execute(&mut database, "INSERT INTO patterns VALUES ('ok')");
    assert!(matches!(
        atomic_error(
            &mut database,
            &format!("UPDATE patterns SET value = '{wide}' WHERE value = 'ok'"),
        ),
        Error::ResourceLimit {
            resource: Resource::RegexBacktracking,
            limit: 10,
        }
    ));
    assert_eq!(
        rows(&mut database, "SELECT value FROM patterns"),
        vec![vec![Value::Text("ok".to_owned())]]
    );

    let mut permissive = Database::new();
    execute(&mut permissive, &declaration);
    execute(
        &mut permissive,
        &format!("INSERT INTO patterns VALUES ('{wide}')"),
    );
    assert!(matches!(
        Database::from_string_with_limits(permissive.into_string(), limits),
        Err(Error::ResourceLimit {
            resource: Resource::RegexBacktracking,
            limit: 10,
        })
    ));
}

#[test]
fn check_like_matches_unicode_scalars() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE unicode_patterns (value TEXT CHECK (value LIKE '__'))",
    );
    execute(&mut database, "INSERT INTO unicode_patterns VALUES ('é😀')");
    assert!(matches!(
        atomic_error(&mut database, "INSERT INTO unicode_patterns VALUES ('é')",),
        Error::Constraint(_)
    ));
}

#[test]
fn check_declaration_errors_are_typed_and_do_not_upgrade_storage() {
    for (sql, expected) in [
        (
            "CREATE TABLE bad (value INTEGER CHECK (missing > 0))",
            "CHECK references unknown column \"missing\" in table \"bad\"",
        ),
        (
            "CREATE TABLE bad (value INTEGER CHECK (value > NULL))",
            "NULL cannot be used as a comparison operand; use IS NULL or IS NOT NULL",
        ),
        (
            "CREATE TABLE bad (value INTEGER CHECK (value > 'wrong'))",
            "CHECK column \"value\" expects INTEGER, got TEXT",
        ),
        (
            "CREATE TABLE bad (value INTEGER CHECK (value LIKE '1%'))",
            "LIKE requires a TEXT column; \"value\" is INTEGER",
        ),
        (
            "CREATE TABLE bad (value INTEGER CHECK (value IN (1, 'wrong')))",
            "CHECK column \"value\" expects INTEGER, got TEXT",
        ),
    ] {
        let mut database = Database::new();
        let error = atomic_error(&mut database, sql);
        assert!(
            matches!(error, Error::Schema(ref message) | Error::Type(ref message) if message == expected),
            "unexpected error for {sql:?}: {error:?}"
        );
        assert_eq!(database.as_str(), "V2;");
    }
}

#[test]
fn check_declaration_diagnostics_follow_source_order() {
    for (sql, expected) in [
        (
            "CREATE TABLE ordered (value INTEGER CHECK (missing > 0) DEFAULT 'wrong')",
            "CHECK references unknown column \"missing\" in table \"ordered\"",
        ),
        (
            "CREATE TABLE ordered (value INTEGER DEFAULT 'wrong' CHECK (missing > 0))",
            "column \"value\" expects INTEGER, got TEXT",
        ),
    ] {
        let mut database = Database::new();
        let error = atomic_error(&mut database, sql);
        assert!(
            matches!(error, Error::Schema(ref message) | Error::Type(ref message) if message == expected),
            "unexpected error for {sql:?}: {error:?}"
        );
        assert_eq!(database.as_str(), "V2;");
    }

    let mut duplicate = Database::new();
    assert!(matches!(
        atomic_error(
            &mut duplicate,
            "CREATE TABLE ordered (value INTEGER CHECK (missing > 0), value INTEGER)",
        ),
        Error::Schema(ref message) if message == "duplicate column name \"value\""
    ));
}

#[test]
fn cumulative_check_predicate_limits_apply_to_create_and_reload() {
    let exact_limits = Limits {
        max_predicates: 3,
        ..Limits::default()
    };
    let mut exact = Database::with_limits(exact_limits.clone());
    execute(
        &mut exact,
        "CREATE TABLE exact_checks (value INTEGER CHECK (value IN (1, 2)), CHECK (value > 0))",
    );
    let blob = exact.into_string();
    Database::from_string_with_limits(blob.clone(), exact_limits)
        .expect("the exact cumulative predicate count reloads");

    let lower_limits = Limits {
        max_predicates: 2,
        ..Limits::default()
    };
    assert!(matches!(
        Database::from_string_with_limits(blob, lower_limits),
        Err(Error::ResourceLimit {
            resource: Resource::CheckPredicates,
            limit: 2,
        })
    ));

    let mut one_over = Database::with_limits(Limits {
        max_predicates: 3,
        ..Limits::default()
    });
    assert!(matches!(
        atomic_error(
            &mut one_over,
            "CREATE TABLE one_over (value INTEGER CHECK (value IN (1, 2)), CHECK (value > 0), CHECK (value IS NOT NULL))",
        ),
        Error::ResourceLimit {
            resource: Resource::CheckPredicates,
            limit: 3,
        }
    ));
    assert_eq!(one_over.as_str(), "V2;");
}

#[test]
fn check_predicate_limits_reset_for_each_table() {
    let limits = Limits {
        max_predicates: 1,
        ..Limits::default()
    };
    let mut database = Database::with_limits(limits.clone());
    execute(
        &mut database,
        "CREATE TABLE first_checks (value INTEGER CHECK (value > 0))",
    );
    execute(
        &mut database,
        "CREATE TABLE second_checks (value INTEGER CHECK (value < 10))",
    );

    Database::from_string_with_limits(database.into_string(), limits)
        .expect("each table receives an independent CHECK predicate budget");
}

#[test]
fn check_storage_preserves_ordered_paginated_queries_after_reload() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE checked_query (id INTEGER CHECK (id > 0), state TEXT)",
    );
    for id in 1..=4 {
        execute(
            &mut database,
            &format!("INSERT INTO checked_query VALUES ({id}, 'ready')"),
        );
    }

    let mut reloaded =
        Database::from_string(database.into_string()).expect("CHECK-bearing storage reloads");
    assert_eq!(
        rows(
            &mut reloaded,
            "SELECT id FROM checked_query WHERE id IN (1, 2, 3, 4) AND id >= 1 ORDER BY id DESC LIMIT 2 OFFSET 1",
        ),
        vec![vec![Value::Integer(3)], vec![Value::Integer(2)]]
    );
}

#[test]
fn check_is_reserved_and_valid_declarations_trigger_v3() {
    let mut database = Database::new();
    for sql in [
        "CREATE TABLE check (value INTEGER)",
        "CREATE TABLE reserved (check INTEGER)",
    ] {
        let error = atomic_error(&mut database, sql);
        assert!(
            matches!(&error, Error::Parse { message, .. }
                if message.contains("reserved keyword `CHECK`")),
            "CHECK must not be usable as an identifier, got {error}"
        );
    }

    execute(
        &mut database,
        "CREATE TABLE checked (value INTEGER CHECK (value != 0))",
    );
    assert!(database.as_str().starts_with("V3;"));
    execute(&mut database, "INSERT INTO checked VALUES (7)");
    assert_eq!(
        rows(&mut database, "SELECT value FROM checked"),
        vec![vec![Value::Integer(7)]]
    );
}

#[test]
fn deeply_nested_check_programs_create_evaluate_and_reload_without_recursion() {
    const PREDICATES: usize = 2_048;
    let mut expression = String::with_capacity(PREDICATES * 20);
    expression.extend(std::iter::repeat_n('(', PREDICATES - 1));
    expression.push_str("value = 0");
    for index in 1..PREDICATES {
        expression.push_str(if index % 2 == 0 { " AND " } else { " OR " });
        expression.push_str("value = 0)");
    }
    let sql = format!("CREATE TABLE deep_check (value INTEGER, CHECK ({expression}))");
    let limits = Limits {
        max_sql_bytes: sql.len(),
        max_database_bytes: 2 * 1024 * 1024,
        max_predicates: PREDICATES,
        ..Limits::default()
    };
    let mut database = Database::with_limits(limits.clone());
    execute(&mut database, &sql);
    execute(&mut database, "INSERT INTO deep_check VALUES (0)");

    let blob = database.into_string();
    let reloaded = Database::from_string_with_limits(blob.clone(), limits)
        .expect("deep CHECK program reloads iteratively");
    assert_eq!(reloaded.as_str(), blob);
}

#[test]
fn escaped_check_metadata_honors_the_exact_database_boundary_atomically() {
    let sql =
        "CREATE TABLE escape_bound (value TEXT CHECK (value LIKE '\\%\\_\\\\|;~\u{2028}\u{2029}'))";
    let mut probe = Database::new();
    execute(&mut probe, sql);
    let expected = probe.into_string();
    assert!(
        expected.contains("|LIKE|0|8|L%000025|L_|L\\|L%00007C|L%00003B|L%00007E|L%002028|L%002029")
    );

    let exact_limits = Limits {
        max_database_bytes: expected.len(),
        ..Limits::default()
    };
    let mut exact = Database::with_limits(exact_limits);
    execute(&mut exact, sql);
    assert_eq!(exact.as_str(), expected);

    let lower_limit = expected.len() - 1;
    let mut lower = Database::with_limits(Limits {
        max_database_bytes: lower_limit,
        ..Limits::default()
    });
    assert!(matches!(
        lower.execute(sql),
        Err(Error::ResourceLimit {
            resource: Resource::DatabaseBytes,
            limit,
        }) if limit == lower_limit
    ));
    assert_eq!(lower.as_str(), "V2;");

    execute(&mut lower, "CREATE TABLE ok (id INTEGER)");
    assert_eq!(lower.as_str(), "V2;~S|ok|id:I:?;");
}
