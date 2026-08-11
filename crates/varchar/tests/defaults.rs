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
fn default_states_persist_and_named_omission_is_distinct_from_explicit_null() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE settings (id INTEGER PRIMARY KEY, absent TEXT, explicit TEXT DEFAULT NULL, typed TEXT DEFAULT 'fallback', enabled BOOLEAN DEFAULT TRUE)",
    );

    assert_eq!(
        database.as_str(),
        "V3;~S|settings|id:I:!|absent:T:?|explicit:T:?|typed:T:?|enabled:B:?;~P|settings|id;~D|settings|explicit|N;~D|settings|typed|Tfallback;~D|settings|enabled|B1;"
    );

    execute(&mut database, "INSERT INTO settings (id) VALUES (1)");
    execute(
        &mut database,
        "INSERT INTO settings (id, typed, enabled) VALUES (2, NULL, FALSE)",
    );

    assert_eq!(
        rows(
            &mut database,
            "SELECT id, absent, explicit, typed, enabled FROM settings",
        ),
        vec![
            vec![
                Value::Integer(1),
                Value::Null,
                Value::Null,
                Value::Text("fallback".to_owned()),
                Value::Boolean(true),
            ],
            vec![
                Value::Integer(2),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Boolean(false),
            ],
        ]
    );
}

#[test]
fn positional_inserts_keep_exact_width_and_primary_keys_may_have_defaults() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE tasks (id INTEGER PRIMARY KEY DEFAULT 7, body TEXT DEFAULT 'queued')",
    );

    assert!(matches!(
        atomic_error(&mut database, "INSERT INTO tasks VALUES (1)"),
        Error::Type(ref message) if message == "table \"tasks\" expects 2 values, got 1"
    ));
    execute(&mut database, "INSERT INTO tasks (body) VALUES ('ready')");

    assert_eq!(
        rows(&mut database, "SELECT id, body FROM tasks"),
        vec![vec![Value::Integer(7), Value::Text("ready".to_owned()),]]
    );
}

#[test]
fn default_is_reserved_and_cannot_name_a_table_or_column() {
    let mut database = Database::new();

    for sql in [
        "CREATE TABLE t (default TEXT)",
        "CREATE TABLE default (id INTEGER)",
    ] {
        assert!(
            matches!(
                atomic_error(&mut database, sql),
                Error::Parse { ref message, .. }
                    if message == "reserved keyword `DEFAULT` cannot be used as an identifier"
            ),
            "expected a reserved-keyword parse error for {sql:?}"
        );
        assert_eq!(database.as_str(), "V2;");
    }

    // The keyword is still usable where the grammar expects it.
    execute(
        &mut database,
        "CREATE TABLE settings (id INTEGER PRIMARY KEY, note TEXT DEFAULT 'fallback')",
    );
    execute(&mut database, "INSERT INTO settings (id) VALUES (1)");
    assert_eq!(
        rows(&mut database, "SELECT note FROM settings"),
        vec![vec![Value::Text("fallback".to_owned())]]
    );
}

#[test]
fn invalid_default_declarations_are_rejected_without_upgrading() {
    for (sql, expected) in [
        (
            "CREATE TABLE t (value INTEGER DEFAULT 1 DEFAULT 1)",
            "duplicate DEFAULT declaration for column \"value\"",
        ),
        (
            "CREATE TABLE t (value INTEGER NOT NULL DEFAULT NULL)",
            "DEFAULT NULL is invalid for NOT NULL column \"t\".\"value\"",
        ),
        (
            "CREATE TABLE t (value INTEGER DEFAULT NULL PRIMARY KEY)",
            "DEFAULT NULL is invalid for NOT NULL column \"t\".\"value\"",
        ),
        (
            "CREATE TABLE t (value INTEGER PRIMARY KEY AUTOINCREMENT DEFAULT 1)",
            "auto-increment column \"t\".\"value\" cannot have a DEFAULT",
        ),
        (
            "CREATE TABLE t (value INTEGER DEFAULT 1 PRIMARY KEY AUTO_INCREMENT)",
            "auto-increment column \"t\".\"value\" cannot have a DEFAULT",
        ),
    ] {
        let mut database = Database::new();
        assert!(matches!(
            atomic_error(&mut database, sql),
            Error::Schema(ref message) if message == expected
        ));
        assert_eq!(database.as_str(), "V2;");
    }

    let mut database = Database::new();
    assert!(matches!(
        atomic_error(
            &mut database,
            "CREATE TABLE t (value INTEGER DEFAULT 'wrong')",
        ),
        Error::Type(ref message)
            if message == "column \"value\" expects INTEGER, got TEXT"
    ));
    assert_eq!(database.as_str(), "V2;");

    assert!(matches!(
        atomic_error(
            &mut database,
            "CREATE TABLE t (value INTEGER DEFAULT other)",
        ),
        Error::Parse { .. }
    ));
}

#[test]
fn defaults_do_not_backfill_rows_and_survive_reload() {
    let blob = String::from(
        "V3;~S|items|id:I:!|note:T:?;~P|items|id;~D|items|note|Tfallback;~R|items|I1|N;",
    );
    let mut database = Database::from_string(blob.clone()).expect("V3 fixture loads");
    assert_eq!(database.as_str(), blob);

    execute(&mut database, "INSERT INTO items (id) VALUES (2)");
    assert_eq!(
        rows(&mut database, "SELECT id, note FROM items"),
        vec![
            vec![Value::Integer(1), Value::Null],
            vec![Value::Integer(2), Value::Text("fallback".to_owned())],
        ]
    );

    let blob = database.into_string();
    let mut reloaded = Database::from_string(blob.clone()).expect("DEFAULT metadata reloads");
    assert_eq!(reloaded.as_str(), blob);
    execute(&mut reloaded, "INSERT INTO items (id) VALUES (3)");
    assert_eq!(
        rows(&mut reloaded, "SELECT note FROM items"),
        vec![
            vec![Value::Null],
            vec![Value::Text("fallback".to_owned())],
            vec![Value::Text("fallback".to_owned())],
        ]
    );
}

#[test]
fn first_v3_upgrade_is_atomic_and_v3_is_sticky() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE legacy (id INTEGER PRIMARY KEY, note TEXT)",
    );
    execute(&mut database, "INSERT INTO legacy VALUES (1, 'kept')");
    let v2 = database.as_str().to_owned();
    assert!(v2.starts_with("V2;"));

    execute(
        &mut database,
        "CREATE TABLE settings (enabled BOOLEAN DEFAULT TRUE)",
    );
    let upgraded = database.as_str().to_owned();
    assert!(upgraded.starts_with("V3;"));
    assert!(upgraded.contains(&v2[3..v2.find("~R|").expect("legacy row exists")]));
    assert!(upgraded.ends_with("~R|legacy|I1|Tkept;"));

    let metadata_end = upgraded.find("~R|").expect("row suffix exists");
    let metadata = upgraded[..metadata_end].to_owned();
    execute(
        &mut database,
        "INSERT INTO settings (enabled) VALUES (FALSE)",
    );
    execute(
        &mut database,
        "UPDATE legacy SET note = 'changed' WHERE id = 1",
    );
    assert!(database.as_str().starts_with("V3;"));
    assert_eq!(&database.as_str()[..metadata_end], metadata);

    let limits = Limits {
        max_database_bytes: 3,
        ..Limits::default()
    };
    let mut constrained = Database::with_limits(limits);
    assert!(matches!(
        atomic_error(
            &mut constrained,
            "CREATE TABLE too_large (value TEXT DEFAULT 'x')",
        ),
        Error::ResourceLimit {
            resource: Resource::DatabaseBytes,
            limit: 3,
        }
    ));
    assert_eq!(constrained.as_str(), "V2;");

    let mut sticky =
        Database::from_string("V3;~S|plain|id:I:!;".to_owned()).expect("legacy-only V3 is valid");
    execute(&mut sticky, "INSERT INTO plain VALUES (1)");
    assert_eq!(sticky.as_str(), "V3;~S|plain|id:I:!;~R|plain|I1;");
}

#[test]
fn complete_v2_fixture_loads_without_rewrite_and_legacy_writes_stay_v2() {
    let fixture = String::from(
        "V2;~S|parents|id:I:!;~P|parents|id;~A|parents|id|I1;~S|children|id:I:!|parent_id:I:?;~P|children|id;~F|children|parent_id|parents|id;~R|parents|I1;~R|children|I10|I1;",
    );
    let mut database = Database::from_string(fixture.clone()).expect("complete V2 fixture loads");
    assert_eq!(database.as_str(), fixture);

    execute(&mut database, "INSERT INTO children VALUES (11, 1)");
    assert!(database.as_str().starts_with("V2;"));
    assert!(!database.as_str().contains("~D|"));
}
