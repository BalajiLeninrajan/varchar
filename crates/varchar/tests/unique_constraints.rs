#![cfg(not(target_family = "wasm"))]

use varchar::{Database, Error, Outcome, Value};

fn execute(database: &mut Database, sql: &str) -> Outcome {
    database
        .execute(sql)
        .unwrap_or_else(|error| panic!("failed to execute {sql:?}: {error}"))
}

fn rows(database: &mut Database, sql: &str) -> Vec<Vec<Value>> {
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
fn inline_and_table_unique_constraints_persist_and_work_after_reload() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE accounts (id INTEGER PRIMARY KEY, email TEXT UNIQUE, handle TEXT, UNIQUE (handle))",
    );
    assert_eq!(
        database.as_str(),
        "V3;~S|accounts|id:I:!|email:T:?|handle:T:?;~P|accounts|id;~U|accounts|email;~U|accounts|handle;"
    );

    execute(
        &mut database,
        "INSERT INTO accounts VALUES (1, 'one@example.com', 'one')",
    );
    execute(
        &mut database,
        "INSERT INTO accounts VALUES (2, 'two@example.com', 'two')",
    );
    assert!(matches!(
        atomic_error(
            &mut database,
            "INSERT INTO accounts VALUES (3, 'one@example.com', 'three')",
        ),
        Error::Constraint(ref message)
            if message == "duplicate UNIQUE value for table \"accounts\" column \"email\""
    ));

    let blob = database.into_string();
    let mut reloaded = Database::from_string(blob.clone()).expect("UNIQUE metadata reloads");
    assert_eq!(reloaded.as_str(), blob);
    assert!(matches!(
        atomic_error(
            &mut reloaded,
            "INSERT INTO accounts VALUES (3, 'three@example.com', 'two')",
        ),
        Error::Constraint(ref message)
            if message == "duplicate UNIQUE value for table \"accounts\" column \"handle\""
    ));
}

#[test]
fn nullable_unique_excludes_null_and_text_equality_is_exact() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE tokens (value TEXT UNIQUE)");
    for sql in [
        "INSERT INTO tokens VALUES (NULL)",
        "INSERT INTO tokens VALUES (NULL)",
        "INSERT INTO tokens VALUES ('Token')",
        "INSERT INTO tokens VALUES ('token')",
        "INSERT INTO tokens VALUES ('é')",
        "INSERT INTO tokens VALUES ('é')",
    ] {
        execute(&mut database, sql);
    }

    assert!(matches!(
        atomic_error(&mut database, "INSERT INTO tokens VALUES ('Token')"),
        Error::Constraint(_)
    ));
    assert_eq!(rows(&mut database, "SELECT value FROM tokens").len(), 6);
}

#[test]
fn unique_tables_need_no_primary_key_and_updates_roll_back_atomically() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE aliases (name TEXT UNIQUE, enabled BOOLEAN)",
    );
    execute(&mut database, "INSERT INTO aliases VALUES ('one', TRUE)");
    execute(&mut database, "INSERT INTO aliases VALUES ('two', FALSE)");

    assert!(matches!(
        atomic_error(&mut database, "UPDATE aliases SET name = 'same'"),
        Error::Constraint(ref message)
            if message == "duplicate UNIQUE value for table \"aliases\" column \"name\""
    ));
    assert_eq!(
        rows(&mut database, "SELECT name FROM aliases"),
        vec![
            vec![Value::Text("one".to_owned())],
            vec![Value::Text("two".to_owned())],
        ]
    );
}

#[test]
fn primary_key_unique_is_normalized_without_a_v3_upgrade() {
    for sql in [
        "CREATE TABLE ids (id INTEGER PRIMARY KEY UNIQUE)",
        "CREATE TABLE ids (id INTEGER UNIQUE, PRIMARY KEY (id))",
        "CREATE TABLE ids (UNIQUE (id), id INTEGER PRIMARY KEY)",
    ] {
        let mut database = Database::new();
        execute(&mut database, sql);
        assert_eq!(database.as_str(), "V2;~S|ids|id:I:!;~P|ids|id;");
    }

    for sql in [
        "CREATE TABLE ids (id INTEGER PRIMARY KEY UNIQUE UNIQUE)",
        "CREATE TABLE ids (id INTEGER PRIMARY KEY UNIQUE, UNIQUE (id))",
    ] {
        let mut database = Database::new();
        assert!(matches!(
            atomic_error(&mut database, sql),
            Error::Schema(ref message)
                if message == "duplicate UNIQUE declaration for column \"id\""
        ));
        assert_eq!(database.as_str(), "V2;");
    }
}

#[test]
fn composite_unique_is_unsupported_and_unique_is_reserved() {
    let mut database = Database::new();
    assert!(matches!(
        atomic_error(
            &mut database,
            "CREATE TABLE pairs (left_value INTEGER, right_value INTEGER, UNIQUE (left_value, right_value))",
        ),
        Error::Unsupported { .. }
    ));

    for sql in [
        "CREATE TABLE t (unique INTEGER)",
        "CREATE TABLE unique (id INTEGER)",
    ] {
        assert!(
            matches!(
                atomic_error(&mut database, sql),
                Error::Parse { ref message, .. }
                    if message == "reserved keyword `UNIQUE` cannot be used as an identifier"
            ),
            "expected a reserved-keyword parse error for {sql:?}"
        );
        assert_eq!(database.as_str(), "V2;");
    }

    // The keyword is still usable where the grammar expects it.
    execute(
        &mut database,
        "CREATE TABLE labels (id INTEGER, value TEXT UNIQUE)",
    );
    execute(&mut database, "INSERT INTO labels VALUES (1, 'kept')");
    assert_eq!(
        rows(&mut database, "SELECT id FROM labels"),
        vec![vec![Value::Integer(1)]]
    );
}
