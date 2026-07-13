#![cfg(not(target_family = "wasm"))]

use varchar::{Database, Error, Outcome, Value};

fn execute(database: &mut Database, sql: &str) -> Outcome {
    database
        .execute(sql)
        .unwrap_or_else(|error| panic!("failed to execute {sql:?}: {error}"))
}

fn values(database: &mut Database, sql: &str) -> Vec<Vec<Value>> {
    match execute(database, sql) {
        Outcome::Rows(rows) => rows.into_rows(),
        other => panic!("expected rows, got {other:?}"),
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
fn both_spellings_generate_for_omitted_and_null_values() {
    for modifier in ["AUTOINCREMENT", "AUTO_INCREMENT"] {
        let mut database = Database::new();
        execute(
            &mut database,
            "CREATE TABLE parents (id INTEGER PRIMARY KEY)",
        );
        execute(&mut database, "INSERT INTO parents VALUES (1)");
        execute(&mut database, "INSERT INTO parents VALUES (2)");
        execute(
            &mut database,
            &format!(
                "CREATE TABLE messages (id INTEGER REFERENCES parents(id) {modifier} PRIMARY KEY NOT NULL, body TEXT NOT NULL)"
            ),
        );
        execute(
            &mut database,
            "INSERT INTO messages (body) VALUES ('omitted')",
        );
        execute(
            &mut database,
            "INSERT INTO messages VALUES (NULL, 'explicit null')",
        );

        assert_eq!(
            values(&mut database, "SELECT id, body FROM messages"),
            vec![
                vec![Value::Integer(1), Value::Text("omitted".to_owned())],
                vec![Value::Integer(2), Value::Text("explicit null".to_owned()),],
            ]
        );
        assert!(database.as_str().contains("~A|messages|id|I2;"));
    }
}

#[test]
fn auto_increment_words_remain_contextual_identifiers() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE autoincrement (auto_increment INTEGER, value INTEGER PRIMARY KEY AUTOINCREMENT)",
    );
    execute(
        &mut database,
        "INSERT INTO autoincrement (auto_increment) VALUES (41)",
    );

    assert_eq!(
        values(
            &mut database,
            "SELECT auto_increment, value FROM autoincrement",
        ),
        vec![vec![Value::Integer(41), Value::Integer(1)]]
    );
}

#[test]
fn high_water_advances_and_never_reuses_deleted_ids_after_reload() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE ids (id INTEGER PRIMARY KEY AUTOINCREMENT)",
    );
    for sql in [
        "INSERT INTO ids VALUES (NULL)",
        "INSERT INTO ids VALUES (10)",
        "INSERT INTO ids VALUES (-5)",
        "INSERT INTO ids VALUES (0)",
        "INSERT INTO ids VALUES (NULL)",
    ] {
        execute(&mut database, sql);
    }
    execute(&mut database, "DELETE FROM ids WHERE id = 11");

    let blob = database.into_string();
    assert!(blob.contains("~A|ids|id|I11;"));
    let mut reloaded = Database::from_string(blob).expect("auto-increment state reloads");
    execute(&mut reloaded, "INSERT INTO ids VALUES (NULL)");

    assert_eq!(
        values(&mut reloaded, "SELECT id FROM ids"),
        vec![
            vec![Value::Integer(1)],
            vec![Value::Integer(10)],
            vec![Value::Integer(-5)],
            vec![Value::Integer(0)],
            vec![Value::Integer(12)],
        ]
    );
    assert!(reloaded.as_str().contains("~A|ids|id|I12;"));
}

#[test]
fn failed_inserts_do_not_consume_generated_ids() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY)",
    );
    execute(&mut database, "INSERT INTO parents VALUES (1)");
    execute(
        &mut database,
        "CREATE TABLE children (id INTEGER PRIMARY KEY AUTOINCREMENT, parent_id INTEGER REFERENCES parents(id), body TEXT NOT NULL)",
    );

    assert!(matches!(
        atomic_error(
            &mut database,
            "INSERT INTO children (parent_id, body) VALUES (999, 'orphan')",
        ),
        Error::Constraint(_)
    ));
    assert!(matches!(
        atomic_error(
            &mut database,
            "INSERT INTO children (parent_id, body) VALUES (1, NULL)",
        ),
        Error::Type(_)
    ));
    execute(
        &mut database,
        "INSERT INTO children (parent_id, body) VALUES (1, 'first')",
    );
    assert!(matches!(
        atomic_error(
            &mut database,
            "INSERT INTO children VALUES (1, 1, 'duplicate')",
        ),
        Error::Constraint(_)
    ));
    execute(
        &mut database,
        "INSERT INTO children (parent_id, body) VALUES (1, 'second')",
    );

    assert_eq!(
        values(&mut database, "SELECT id FROM children"),
        vec![vec![Value::Integer(1)], vec![Value::Integer(2)]]
    );
}

#[test]
fn sequence_exhaustion_is_atomic() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE ids (id INTEGER PRIMARY KEY AUTOINCREMENT)",
    );
    execute(
        &mut database,
        &format!("INSERT INTO ids VALUES ({})", i64::MAX),
    );

    assert!(matches!(
        atomic_error(&mut database, "INSERT INTO ids VALUES (NULL)"),
        Error::Constraint(_)
    ));
    assert_eq!(
        values(&mut database, "SELECT id FROM ids"),
        vec![vec![Value::Integer(i64::MAX)]]
    );
}

#[test]
fn successful_updates_advance_but_zero_match_and_failed_updates_do_not() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE ids (id INTEGER PRIMARY KEY AUTOINCREMENT)",
    );
    execute(&mut database, "INSERT INTO ids VALUES (NULL)");
    execute(&mut database, "INSERT INTO ids VALUES (NULL)");
    execute(&mut database, "UPDATE ids SET id = 10 WHERE id = 2");
    assert_eq!(
        execute(&mut database, "UPDATE ids SET id = 20 WHERE id = 999"),
        Outcome::Affected { rows: 0 }
    );
    assert!(matches!(
        atomic_error(&mut database, "UPDATE ids SET id = 1 WHERE id = 10"),
        Error::Constraint(_)
    ));
    execute(&mut database, "INSERT INTO ids VALUES (NULL)");

    assert_eq!(
        values(&mut database, "SELECT id FROM ids"),
        vec![
            vec![Value::Integer(1)],
            vec![Value::Integer(10)],
            vec![Value::Integer(11)],
        ]
    );
    assert!(database.as_str().contains("~A|ids|id|I11;"));
}

#[test]
fn invalid_auto_increment_definitions_are_schema_errors() {
    let mut database = Database::new();
    for sql in [
        "CREATE TABLE text_ids (id TEXT PRIMARY KEY AUTOINCREMENT)",
        "CREATE TABLE no_key (id INTEGER AUTOINCREMENT)",
        "CREATE TABLE two_auto (a INTEGER PRIMARY KEY AUTOINCREMENT, b INTEGER AUTOINCREMENT)",
        "CREATE TABLE duplicate_auto (id INTEGER PRIMARY KEY AUTOINCREMENT AUTO_INCREMENT)",
    ] {
        assert!(matches!(atomic_error(&mut database, sql), Error::Schema(_)));
    }
}

#[test]
fn malformed_or_stale_auto_increment_records_are_rejected() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE ids (id INTEGER PRIMARY KEY AUTOINCREMENT)",
    );
    execute(&mut database, "INSERT INTO ids VALUES (NULL)");
    let blob = database.into_string();
    let record = "~A|ids|id|I1;";
    assert!(blob.contains(record));

    for replacement in [
        "~A|ids|id|I-1;",
        "~A|ids|id|I0;",
        "~A|missing|id|I1;",
        "~A|ids|missing|I1;",
        "~A|ids|id|I01;",
        "~A|ids|id|T1;",
        "~A|ids|id|I1;~A|ids|id|I1;",
    ] {
        let corrupt = blob.replacen(record, replacement, 1);
        assert!(
            matches!(
                Database::from_string(corrupt),
                Err(Error::CorruptStorage { .. })
            ),
            "accepted replacement {replacement:?}"
        );
    }

    let reordered = blob.replacen("~P|ids|id;~A|ids|id|I1;", "~A|ids|id|I1;~P|ids|id;", 1);
    assert!(matches!(
        Database::from_string(reordered),
        Err(Error::CorruptStorage { .. })
    ));

    let after_rows = format!("{}{record}", blob.replacen(record, "", 1));
    assert!(matches!(
        Database::from_string(after_rows),
        Err(Error::CorruptStorage { .. })
    ));
}
