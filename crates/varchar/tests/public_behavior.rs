#![cfg(not(target_family = "wasm"))]

use varchar::{Column, DataType, Database, Error, Limits, Outcome, RowSet, Value};

fn execute(database: &mut Database, sql: &str) -> Outcome {
    database
        .execute(sql)
        .unwrap_or_else(|error| panic!("failed to execute {sql:?}: {error}"))
}

fn sql_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn column(name: &str, data_type: DataType, nullable: bool) -> Column {
    Column {
        name: name.to_owned(),
        data_type,
        nullable,
    }
}

fn row_set(outcome: Outcome) -> RowSet {
    match outcome {
        Outcome::Rows(rows) => rows,
        other => panic!("expected rows, got {other:?}"),
    }
}

fn single_text_column(database: &mut Database, sql: &str) -> Vec<Value> {
    let rows = row_set(execute(database, sql));
    assert_eq!(rows.columns.len(), 1);
    rows.rows
        .into_iter()
        .map(|row| {
            assert_eq!(row.len(), 1);
            row.into_iter().next().expect("one projected value")
        })
        .collect()
}

#[test]
fn typed_crud_and_optional_insert_columns() {
    let mut database = Database::new();
    assert_eq!(database.as_str(), "V1;");

    assert_eq!(
        execute(
            &mut database,
            "CREATE TABLE Things (ID INTEGER NOT NULL, Note TEXT, Active BOOLEAN NOT NULL)",
        ),
        Outcome::Created {
            table: "things".to_owned(),
        }
    );
    assert_eq!(
        execute(
            &mut database,
            "INSERT INTO things VALUES (1, 'first', TRUE)",
        ),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        execute(
            &mut database,
            "INSERT INTO THINGS (active, id) VALUES (FALSE, 2)",
        ),
        Outcome::Affected { rows: 1 }
    );

    assert_eq!(
        row_set(execute(
            &mut database,
            "SELECT id, note, active FROM things",
        )),
        RowSet {
            columns: vec![
                column("id", DataType::Integer, false),
                column("note", DataType::Text, true),
                column("active", DataType::Boolean, false),
            ],
            rows: vec![
                vec![
                    Value::Integer(1),
                    Value::Text("first".to_owned()),
                    Value::Boolean(true),
                ],
                vec![Value::Integer(2), Value::Null, Value::Boolean(false)],
            ],
        }
    );

    assert_eq!(
        execute(
            &mut database,
            "UPDATE things SET note = 'filled', active = TRUE WHERE id = 2 AND note IS NULL",
        ),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        execute(
            &mut database,
            "UPDATE things SET note = 'unused' WHERE id = 999",
        ),
        Outcome::Affected { rows: 0 }
    );
    assert_eq!(
        execute(
            &mut database,
            "DELETE FROM things WHERE active = TRUE AND id != 2",
        ),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        execute(&mut database, "DELETE FROM things WHERE id = 999"),
        Outcome::Affected { rows: 0 }
    );

    assert_eq!(
        row_set(execute(&mut database, "SELECT * FROM things")),
        RowSet {
            columns: vec![
                column("id", DataType::Integer, false),
                column("note", DataType::Text, true),
                column("active", DataType::Boolean, false),
            ],
            rows: vec![vec![
                Value::Integer(2),
                Value::Text("filled".to_owned()),
                Value::Boolean(true),
            ]],
        }
    );

    assert_eq!(
        execute(&mut database, "DELETE FROM things"),
        Outcome::Affected { rows: 1 }
    );
    let empty = row_set(execute(&mut database, "SELECT * FROM things"));
    assert!(empty.rows.is_empty());
    assert_eq!(empty.columns.len(), 3);
}

#[test]
fn preserves_duplicate_rows_and_projection_order() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE entries (id INTEGER NOT NULL, name TEXT NOT NULL)",
    );
    for sql in [
        "INSERT INTO entries VALUES (1, 'same')",
        "INSERT INTO entries VALUES (1, 'same')",
        "INSERT INTO entries VALUES (2, 'later')",
    ] {
        assert_eq!(execute(&mut database, sql), Outcome::Affected { rows: 1 });
    }

    assert_eq!(
        row_set(execute(&mut database, "SELECT name, id, name FROM entries",)),
        RowSet {
            columns: vec![
                column("name", DataType::Text, false),
                column("id", DataType::Integer, false),
                column("name", DataType::Text, false),
            ],
            rows: vec![
                vec![
                    Value::Text("same".to_owned()),
                    Value::Integer(1),
                    Value::Text("same".to_owned()),
                ],
                vec![
                    Value::Text("same".to_owned()),
                    Value::Integer(1),
                    Value::Text("same".to_owned()),
                ],
                vec![
                    Value::Text("later".to_owned()),
                    Value::Integer(2),
                    Value::Text("later".to_owned()),
                ],
            ],
        }
    );

    assert_eq!(
        execute(
            &mut database,
            "UPDATE entries SET name = 'changed' WHERE id = 1",
        ),
        Outcome::Affected { rows: 2 }
    );
    assert_eq!(
        execute(&mut database, "DELETE FROM entries WHERE id = 1"),
        Outcome::Affected { rows: 2 }
    );
}

#[test]
fn compile_and_explain_expose_the_same_regex_plan_without_mutating() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE users (id INTEGER NOT NULL, name TEXT, active BOOLEAN)",
    );
    execute(&mut database, "INSERT INTO users VALUES (7, 'Ada', TRUE)");
    let select = "SELECT name, id, name FROM users WHERE active = TRUE AND name LIKE 'A%'";
    let before = database.as_str().to_owned();

    let plan = database.compile_select(select).expect("SELECT compiles");
    assert_eq!(plan.table(), "users");
    assert!(!plan.pattern().is_empty());
    assert_eq!(
        plan.columns(),
        vec![
            column("name", DataType::Text, true),
            column("id", DataType::Integer, false),
            column("name", DataType::Text, true),
        ]
    );
    assert_eq!(database.as_str(), before);

    assert_eq!(
        execute(&mut database, &format!("EXPLAIN REGEX {select}")),
        Outcome::Explain(plan)
    );
    assert_eq!(database.as_str(), before);

    let selected = row_set(execute(&mut database, select));
    assert_eq!(
        selected.rows,
        vec![vec![
            Value::Text("Ada".to_owned()),
            Value::Integer(7),
            Value::Text("Ada".to_owned()),
        ]]
    );
    assert_eq!(database.as_str(), before);
}

#[test]
fn null_predicates_and_comparison_semantics_are_sql_like() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE values_ (id INTEGER NOT NULL, note TEXT)",
    );
    execute(&mut database, "INSERT INTO values_ VALUES (1, NULL)");
    execute(&mut database, "INSERT INTO values_ VALUES (2, 'x')");
    execute(&mut database, "INSERT INTO values_ VALUES (3, 'y')");

    assert_eq!(
        row_set(execute(
            &mut database,
            "SELECT id FROM values_ WHERE note IS NULL",
        ))
        .rows,
        vec![vec![Value::Integer(1)]]
    );
    assert_eq!(
        row_set(execute(
            &mut database,
            "SELECT id FROM values_ WHERE note IS NOT NULL",
        ))
        .rows,
        vec![vec![Value::Integer(2)], vec![Value::Integer(3)]]
    );
    assert_eq!(
        row_set(execute(
            &mut database,
            "SELECT id FROM values_ WHERE note != 'x'",
        ))
        .rows,
        vec![vec![Value::Integer(3)]]
    );
    assert_eq!(
        row_set(execute(
            &mut database,
            "SELECT id FROM values_ WHERE note LIKE '%'",
        ))
        .rows,
        vec![vec![Value::Integer(2)], vec![Value::Integer(3)]]
    );

    for sql in [
        "SELECT id FROM values_ WHERE note = NULL",
        "SELECT id FROM values_ WHERE note != NULL",
        "SELECT id FROM values_ WHERE id LIKE '1'",
    ] {
        assert!(
            matches!(database.execute(sql), Err(Error::Type(_))),
            "expected a type error for {sql:?}"
        );
    }
}

#[test]
fn integer_boundaries_and_boolean_literals_round_trip() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE bounds (n INTEGER NOT NULL, flag BOOLEAN NOT NULL)",
    );
    execute(
        &mut database,
        &format!("INSERT INTO bounds VALUES ({}, TrUe)", i64::MIN),
    );
    execute(
        &mut database,
        &format!("INSERT INTO bounds VALUES ({}, fAlSe)", i64::MAX),
    );

    assert_eq!(
        row_set(execute(&mut database, "SELECT * FROM bounds")).rows,
        vec![
            vec![Value::Integer(i64::MIN), Value::Boolean(true)],
            vec![Value::Integer(i64::MAX), Value::Boolean(false)],
        ]
    );

    assert!(matches!(
        database.execute("INSERT INTO bounds VALUES (9223372036854775808, TRUE)"),
        Err(Error::Type(_)) | Err(Error::Parse { .. })
    ));
}

#[test]
fn unicode_reserved_characters_and_regex_metacharacters_round_trip() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE strings (value TEXT NOT NULL)");
    let values = [
        "",
        "O'Brien",
        "|;~%\\\n\t\0",
        ".*+?^$[](){}|\\",
        "💾 café e\u{301} \u{2028} \u{2029}",
    ];
    for value in values {
        execute(
            &mut database,
            &format!("INSERT INTO strings VALUES ({})", sql_text(value)),
        );
    }

    let blob = database.as_str().to_owned();
    assert!(blob.starts_with("V1;"));
    assert!(
        !blob.chars().any(char::is_control),
        "storage must remain a printable single line: {blob:?}"
    );
    assert!(!blob.contains(['\u{2028}', '\u{2029}']));

    let mut reloaded = Database::from_string(blob.clone()).expect("canonical blob reloads");
    assert_eq!(reloaded.as_str(), blob);
    assert_eq!(
        single_text_column(&mut reloaded, "SELECT value FROM strings"),
        values
            .iter()
            .map(|value| Value::Text((*value).to_owned()))
            .collect::<Vec<_>>()
    );

    let metacharacters = ".*+?^$[](){}|\\";
    assert_eq!(
        single_text_column(
            &mut reloaded,
            &format!(
                "SELECT value FROM strings WHERE value = {}",
                sql_text(metacharacters)
            ),
        ),
        vec![Value::Text(metacharacters.to_owned())]
    );
}

#[test]
fn like_uses_unicode_scalars_and_honors_escapes() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE patterns (value TEXT)");
    let values = [
        Some("ab"),
        Some("acb"),
        Some("a💾b"),
        Some("aéb"),
        Some("ae\u{301}b"),
        Some("a_b"),
        Some("a%b"),
        Some("a\\b"),
        Some("Ab"),
        None,
    ];
    for value in values {
        let literal = value.map_or_else(|| "NULL".to_owned(), sql_text);
        execute(
            &mut database,
            &format!("INSERT INTO patterns VALUES ({literal})"),
        );
    }

    assert_eq!(
        single_text_column(
            &mut database,
            "SELECT value FROM patterns WHERE value LIKE 'a_b'",
        ),
        ["acb", "a💾b", "aéb", "a_b", "a%b", "a\\b"]
            .into_iter()
            .map(|value| Value::Text(value.to_owned()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        single_text_column(
            &mut database,
            "SELECT value FROM patterns WHERE value LIKE 'a%b'",
        ),
        [
            "ab",
            "acb",
            "a💾b",
            "aéb",
            "ae\u{301}b",
            "a_b",
            "a%b",
            "a\\b",
        ]
        .into_iter()
        .map(|value| Value::Text(value.to_owned()))
        .collect::<Vec<_>>()
    );
    assert_eq!(
        single_text_column(
            &mut database,
            r"SELECT value FROM patterns WHERE value LIKE 'a\_b'",
        ),
        vec![Value::Text("a_b".to_owned())]
    );
    assert_eq!(
        single_text_column(
            &mut database,
            r"SELECT value FROM patterns WHERE value LIKE 'a\%b'",
        ),
        vec![Value::Text("a%b".to_owned())]
    );
    assert_eq!(
        single_text_column(
            &mut database,
            r"SELECT value FROM patterns WHERE value LIKE 'a\\b'",
        ),
        vec![Value::Text("a\\b".to_owned())]
    );
    assert!(
        single_text_column(
            &mut database,
            "SELECT value FROM patterns WHERE value LIKE 'ab_'",
        )
        .is_empty()
    );
    assert!(
        single_text_column(
            &mut database,
            "SELECT value FROM patterns WHERE value LIKE 'ab'",
        )
        .contains(&Value::Text("ab".to_owned()))
    );

    for pattern in [r"'abc\'", r"'abc\q'"] {
        let sql = format!("SELECT value FROM patterns WHERE value LIKE {pattern}");
        assert!(matches!(database.execute(&sql), Err(Error::Type(_))));
    }
}

#[test]
fn regex_matching_stays_within_exact_table_and_row_boundaries() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE t (value TEXT NOT NULL)");
    execute(&mut database, "CREATE TABLE tt (value TEXT NOT NULL)");
    execute(&mut database, "INSERT INTO t VALUES ('left')");
    execute(&mut database, "INSERT INTO tt VALUES ('wrong')");
    execute(&mut database, "INSERT INTO t VALUES ('right')");

    assert_eq!(
        single_text_column(&mut database, "SELECT value FROM t WHERE value LIKE '%'"),
        vec![
            Value::Text("left".to_owned()),
            Value::Text("right".to_owned()),
        ]
    );
}

#[test]
fn unsupported_and_malformed_sql_are_rejected_with_spans() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE t (id INTEGER)");

    let rejected = [
        "",
        "   ",
        "SELECT",
        "SELECT * FROM t WHERE id =",
        "SELECT * FROM t; SELECT * FROM t",
        "SELECT * FROM t WHERE id = 1 OR id = 2",
        "SELECT * FROM t ORDER BY id",
        "SELECT * FROM t JOIN t AS other ON t.id = other.id",
        "SELECT \"id\" FROM t",
        "SELECT * FROM t -- comment",
        "ALTER TABLE t ADD COLUMN name TEXT",
        "SELECT * FROM t AS alias",
    ];

    for sql in rejected {
        let before = database.as_str().to_owned();
        let error = database
            .execute(sql)
            .unwrap_err_or_else(|| panic!("unexpectedly accepted {sql:?}"));
        match error {
            Error::Parse {
                span_start,
                span_end,
                ..
            }
            | Error::Unsupported {
                span_start,
                span_end,
                ..
            } => {
                assert!(span_start <= span_end);
                assert!(span_end <= sql.len());
            }
            other => panic!("expected parse/unsupported error for {sql:?}, got {other:?}"),
        }
        assert_eq!(database.as_str(), before);
    }
}

#[test]
fn every_failed_mutation_is_byte_for_byte_atomic() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE t (id INTEGER NOT NULL, name TEXT NOT NULL)",
    );
    execute(&mut database, "INSERT INTO t VALUES (1, 'kept')");

    let invalid = [
        "INSERT INTO t VALUES ('wrong', 'type')",
        "INSERT INTO t VALUES (2, NULL)",
        "INSERT INTO missing VALUES (1)",
        "INSERT INTO t VALUES (2)",
        "UPDATE t SET id = 'wrong' WHERE id = 1",
        "UPDATE t SET missing = 1 WHERE id = 1",
        "DELETE FROM t WHERE id = NULL",
        "DELETE FROM t WHERE id = 1 OR id = 2",
    ];

    for sql in invalid {
        let before = database.as_str().to_owned();
        assert!(
            database.execute(sql).is_err(),
            "unexpectedly accepted {sql:?}"
        );
        assert_eq!(
            database.as_str(),
            before,
            "mutation was not atomic: {sql:?}"
        );
    }

    assert_eq!(
        row_set(execute(&mut database, "SELECT * FROM t")).rows,
        vec![vec![Value::Integer(1), Value::Text("kept".to_owned()),]]
    );
}

#[test]
fn canonical_storage_is_strictly_validated() {
    let corrupt = [
        "",
        "V0;",
        "V1",
        "V1;garbage",
        "V1;~X|t;",
        "V1;~S|t|v:T:?",
        "V1;~S|T|v:T:?;",
        "V1;~S|t|v:T:?;~S|t|v:T:?;",
        "V1;~S|t|v:T:?|v:I:?;",
        "V1;~R|t|Tx;",
        "V1;~S|t|v:T:?;~R|t;",
        "V1;~S|t|v:T:?;~R|t|I1;",
        "V1;~S|t|v:I:?;~R|t|I01;",
        "V1;~S|t|v:I:?;~R|t|I9223372036854775808;",
        "V1;~S|t|v:B:?;~R|t|B2;",
        "V1;~S|t|v:T:?;~R|t|T%0000zz;",
        "V1;~S|t|v:T:?;~R|t|T%00007C;~S|later|v:T:?;",
    ];

    for blob in corrupt {
        assert!(
            matches!(
                Database::from_string(blob.to_owned()),
                Err(Error::CorruptStorage { .. })
            ),
            "unexpectedly accepted corrupt blob {blob:?}"
        );
    }

    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE t (v TEXT)");
    execute(&mut database, "INSERT INTO t VALUES ('valid')");
    let blob = database.into_string();
    assert_eq!(
        Database::from_string(blob.clone())
            .expect("valid storage reloads")
            .into_string(),
        blob
    );
}

#[test]
fn known_v1_storage_fixture_remains_compatible() {
    let blob =
        "V1;~S|people|id:I:!|note:T:?|active:B:!;~R|people|I-7|Tsemi%00003Bline%002028break|B1;";
    let mut database = Database::from_string(blob.to_owned()).expect("known V1 fixture loads");

    assert_eq!(
        row_set(execute(
            &mut database,
            "SELECT id, note, active FROM people",
        )),
        RowSet {
            columns: vec![
                column("id", DataType::Integer, false),
                column("note", DataType::Text, true),
                column("active", DataType::Boolean, false),
            ],
            rows: vec![vec![
                Value::Integer(-7),
                Value::Text("semi;line\u{2028}break".to_owned()),
                Value::Boolean(true),
            ]],
        }
    );
    assert_eq!(
        execute(&mut database, "INSERT INTO people VALUES (0, NULL, FALSE)",),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(database.into_string(), format!("{blob}~R|people|I0|N|B0;"));
}

#[test]
fn storage_edits_preserve_canonical_record_order() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE t (id INTEGER NOT NULL, name TEXT NOT NULL)",
    );
    execute(&mut database, "INSERT INTO t VALUES (1, 'first')");
    execute(&mut database, "INSERT INTO t VALUES (2, 'second')");
    execute(&mut database, "CREATE TABLE u (flag BOOLEAN NOT NULL)");
    assert_eq!(
        database.as_str(),
        "V1;~S|t|id:I:!|name:T:!;~S|u|flag:B:!;~R|t|I1|Tfirst;~R|t|I2|Tsecond;"
    );

    execute(&mut database, "INSERT INTO u VALUES (TRUE)");
    execute(&mut database, "UPDATE t SET name = 'changed' WHERE id = 1");
    execute(&mut database, "DELETE FROM t WHERE id = 2");
    assert_eq!(
        database.as_str(),
        "V1;~S|t|id:I:!|name:T:!;~S|u|flag:B:!;~R|t|I1|Tchanged;~R|u|B1;"
    );
}

#[test]
fn corrupt_storage_offsets_point_to_the_exact_cell_payload() {
    let blob = "V1;~S|t|id:I:?|note:T:?;~R|t|I1|Tok%0000zz;";
    let expected = blob.find('%').expect("malformed escape is present");

    assert!(matches!(
        Database::from_string(blob.to_owned()),
        Err(Error::CorruptStorage { offset, .. }) if offset == expected
    ));
}

#[test]
fn configurable_resource_limits_fail_without_partial_work() {
    let sql_limits = Limits {
        max_sql_bytes: 8,
        ..Limits::default()
    };
    let mut database = Database::with_limits(sql_limits);
    assert!(matches!(
        database.execute("CREATE TABLE t (id INTEGER)"),
        Err(Error::ResourceLimit { limit: 8, .. })
    ));
    assert_eq!(database.as_str(), "V1;");

    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE t (id INTEGER, name TEXT)");
    execute(&mut database, "INSERT INTO t VALUES (12345, 'a result')");
    let blob = database.into_string();

    let predicate_limits = Limits {
        max_predicates: 1,
        ..Limits::default()
    };
    let database = Database::from_string_with_limits(blob.clone(), predicate_limits)
        .expect("blob fits predicate limits");
    assert!(matches!(
        database.compile_select("SELECT * FROM t WHERE id = 1 AND name = 'x'"),
        Err(Error::ResourceLimit { limit: 1, .. })
    ));

    let pattern_limits = Limits {
        max_pattern_bytes: 1,
        ..Limits::default()
    };
    let database = Database::from_string_with_limits(blob.clone(), pattern_limits)
        .expect("blob fits pattern limits");
    assert!(matches!(
        database.compile_select("SELECT * FROM t"),
        Err(Error::ResourceLimit { limit: 1, .. })
    ));

    let result_limits = Limits {
        max_result_bytes: 1,
        ..Limits::default()
    };
    let mut database = Database::from_string_with_limits(blob.clone(), result_limits)
        .expect("blob fits result limits");
    let before = database.as_str().to_owned();
    assert!(matches!(
        database.execute("SELECT * FROM t"),
        Err(Error::ResourceLimit { limit: 1, .. })
    ));
    assert_eq!(database.as_str(), before);

    let mut null_database = Database::new();
    execute(&mut null_database, "CREATE TABLE nulls (v TEXT)");
    for _ in 0..8 {
        execute(&mut null_database, "INSERT INTO nulls VALUES (NULL)");
    }
    let null_blob = null_database.into_string();
    let null_result_limits = Limits {
        max_result_bytes: 256,
        ..Limits::default()
    };
    let mut null_database = Database::from_string_with_limits(null_blob, null_result_limits)
        .expect("NULL fixture fits database limits");
    assert!(matches!(
        null_database.execute("SELECT * FROM nulls"),
        Err(Error::ResourceLimit {
            resource: "result bytes",
            limit: 256
        })
    ));

    let load_limits = Limits {
        max_database_bytes: blob.len() - 1,
        ..Limits::default()
    };
    assert!(matches!(
        Database::from_string_with_limits(blob.clone(), load_limits),
        Err(Error::ResourceLimit { .. })
    ));

    let database_limit = blob.len();
    let mutation_limits = Limits {
        max_database_bytes: database_limit,
        ..Limits::default()
    };
    let mut database = Database::from_string_with_limits(blob, mutation_limits)
        .expect("existing blob exactly fits");
    for sql in [
        "CREATE TABLE extra (value BOOLEAN)",
        "INSERT INTO t VALUES (2, 'too large')",
        "UPDATE t SET name = 'a much larger result' WHERE id = 12345",
    ] {
        let before = database.as_str().to_owned();
        assert!(matches!(
            database.execute(sql),
            Err(Error::ResourceLimit {
                resource: "database bytes",
                limit,
            }) if limit == database_limit
        ));
        assert_eq!(database.as_str(), before, "failed mutation changed {sql:?}");
    }
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
