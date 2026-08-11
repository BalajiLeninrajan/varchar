#![cfg(not(target_family = "wasm"))]

use varchar::{Database, Error, Limits, Outcome, Resource, Value};

fn execute(database: &mut Database, sql: &str) -> Outcome {
    database
        .execute(sql)
        .unwrap_or_else(|error| panic!("failed to execute {sql:?}: {error}"))
}

fn atomic_error(database: &mut Database, sql: &str) -> Error {
    let before = database.as_str().to_owned();
    let error = database
        .execute(sql)
        .unwrap_err_or_else(|| panic!("unexpectedly accepted {sql:?}"));
    assert_eq!(database.as_str(), before, "failed mutation changed state");
    error
}

fn rows(database: &mut Database, sql: &str) -> Vec<Vec<Value>> {
    match execute(database, sql) {
        Outcome::Rows(rows) => rows.into_rows(),
        other => panic!("expected rows, got {other:?}"),
    }
}

fn alternating_check(depth: usize) -> String {
    let mut expression = String::new();
    for level in 0..depth {
        expression.push_str("id >= 0 ");
        expression.push_str(if level % 2 == 0 { "AND" } else { "OR" });
        expression.push_str(" (");
    }
    expression.push_str("id >= 0");
    expression.extend(std::iter::repeat_n(')', depth));
    expression
}

#[test]
fn length_changing_updates_keep_every_original_row_identity() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE items (id INTEGER PRIMARY KEY, body TEXT NOT NULL)",
    );
    for sql in [
        "INSERT INTO items VALUES (1, 'a')",
        "INSERT INTO items VALUES (2, 'medium')",
        "INSERT INTO items VALUES (3, 'z')",
    ] {
        execute(&mut database, sql);
    }

    assert_eq!(
        execute(
            &mut database,
            "UPDATE items SET body = 'a much longer replacement'",
        ),
        Outcome::Affected { rows: 3 }
    );
    assert_eq!(
        database.as_str(),
        "V2;~S|items|id:I:!|body:T:!;~P|items|id;~R|items|I1|Ta much longer replacement;~R|items|I2|Ta much longer replacement;~R|items|I3|Ta much longer replacement;"
    );

    assert_eq!(
        execute(&mut database, "DELETE FROM items WHERE id != 2"),
        Outcome::Affected { rows: 2 }
    );
    assert_eq!(
        database.as_str(),
        "V2;~S|items|id:I:!|body:T:!;~P|items|id;~R|items|I2|Ta much longer replacement;"
    );
}

#[test]
fn zero_match_mutations_preserve_exact_bytes_and_sequence_state() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE ids (id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT NOT NULL)",
    );
    execute(&mut database, "INSERT INTO ids (body) VALUES ('one')");
    let before = database.as_str().to_owned();

    assert_eq!(
        execute(&mut database, "UPDATE ids SET id = 50 WHERE id = 999"),
        Outcome::Affected { rows: 0 }
    );
    assert_eq!(database.as_str(), before);
    assert_eq!(
        execute(&mut database, "DELETE FROM ids WHERE id = 999"),
        Outcome::Affected { rows: 0 }
    );
    assert_eq!(database.as_str(), before);

    execute(&mut database, "INSERT INTO ids (body) VALUES ('two')");
    assert_eq!(
        rows(&mut database, "SELECT id, body FROM ids"),
        vec![
            vec![Value::Integer(1), Value::Text(String::from("one"))],
            vec![Value::Integer(2), Value::Text(String::from("two"))],
        ]
    );
    assert!(database.as_str().contains("~A|ids|id|I2;"));
}

#[test]
fn deep_check_metadata_does_not_participate_in_sequence_replacement_encoding() {
    let check = alternating_check(512);
    let limits = Limits {
        max_predicates: 1_024,
        ..Limits::default()
    };
    let mut database = Database::with_limits(limits.clone());
    execute(
        &mut database,
        &format!("CREATE TABLE deep_ids (id INTEGER PRIMARY KEY AUTOINCREMENT, CHECK ({check}))"),
    );
    execute(&mut database, "INSERT INTO deep_ids VALUES (NULL)");
    let before = database.as_str().to_owned();

    assert_eq!(
        execute(&mut database, "UPDATE deep_ids SET id = 50 WHERE id = 999"),
        Outcome::Affected { rows: 0 }
    );
    assert_eq!(database.as_str(), before);

    assert_eq!(
        execute(&mut database, "UPDATE deep_ids SET id = 2 WHERE id = 1"),
        Outcome::Affected { rows: 1 }
    );
    assert!(database.as_str().contains("~A|deep_ids|id|I2;"));
    assert_eq!(
        rows(&mut database, "SELECT id FROM deep_ids"),
        vec![vec![Value::Integer(2)]]
    );
    Database::from_string_with_limits(database.into_string(), limits)
        .expect("the complete candidate still passes final metadata validation");
}

#[test]
fn candidate_validation_keeps_primary_key_precedence_and_rolls_back_sequence() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY)",
    );
    execute(&mut database, "INSERT INTO parents VALUES (1)");
    execute(
        &mut database,
        "CREATE TABLE children (id INTEGER PRIMARY KEY AUTOINCREMENT, parent_id INTEGER REFERENCES parents(id))",
    );
    execute(&mut database, "INSERT INTO children (parent_id) VALUES (1)");
    execute(&mut database, "INSERT INTO children (parent_id) VALUES (1)");

    let error = atomic_error(
        &mut database,
        "UPDATE children SET id = 100, parent_id = 999",
    );
    match error {
        Error::Constraint(message) => {
            assert_eq!(message, "duplicate primary key in table \"children\"")
        }
        other => panic!("expected a primary-key constraint error, got {other:?}"),
    }

    execute(&mut database, "INSERT INTO children (parent_id) VALUES (1)");
    assert_eq!(
        rows(&mut database, "SELECT id FROM children"),
        vec![
            vec![Value::Integer(1)],
            vec![Value::Integer(2)],
            vec![Value::Integer(3)],
        ]
    );
    assert!(database.as_str().contains("~A|children|id|I3;"));
}

#[test]
fn legacy_v2_metadata_and_rows_keep_their_exact_physical_bytes() {
    const PREFIX: &str = "V2;~S|parents|id:I:!;~P|parents|id;~S|children|id:I:!|parent_id:I:?|note:T:!;~P|children|id;~F|children|parent_id|parents|id;~A|children|id|I2;";
    let blob = format!("{PREFIX}~R|parents|I1;~R|children|I1|I1|Tone;~R|children|I2|N|Ttwo;");
    let mut database =
        Database::from_string(blob.clone()).expect("the complete legacy V2 fixture loads");
    assert_eq!(database.as_str(), blob);

    assert_eq!(
        execute(
            &mut database,
            "UPDATE children SET note = 'a longer replacement' WHERE id = 1",
        ),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        database.as_str(),
        format!(
            "{PREFIX}~R|parents|I1;~R|children|I1|I1|Ta longer replacement;~R|children|I2|N|Ttwo;"
        )
    );

    let mut reloaded = Database::from_string(database.into_string())
        .expect("the planned update reloads without normalizing metadata");
    assert_eq!(
        execute(&mut reloaded, "DELETE FROM children WHERE id = 2"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        reloaded.as_str(),
        format!("{PREFIX}~R|parents|I1;~R|children|I1|I1|Ta longer replacement;")
    );
}

#[test]
fn database_size_failure_rolls_back_rows_and_sequence_state() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE ids (id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT NOT NULL)",
    );
    execute(&mut database, "INSERT INTO ids (body) VALUES ('one')");

    let blob = database.into_string();
    let limit = blob.len();
    let limits = Limits {
        max_database_bytes: limit,
        ..Limits::default()
    };
    let mut limited = Database::from_string_with_limits(blob.clone(), limits)
        .expect("the source exactly fits its database limit");

    assert!(matches!(
        atomic_error(
            &mut limited,
            "UPDATE ids SET id = 100, body = 'a much longer replacement' WHERE id = 1",
        ),
        Error::ResourceLimit {
            resource: Resource::DatabaseBytes,
            limit: actual,
        } if actual == limit
    ));
    assert_eq!(limited.as_str(), blob);

    let mut reloaded =
        Database::from_string(limited.into_string()).expect("the rolled-back state reloads");
    execute(&mut reloaded, "INSERT INTO ids (body) VALUES ('two')");
    assert_eq!(
        rows(&mut reloaded, "SELECT id, body FROM ids"),
        vec![
            vec![Value::Integer(1), Value::Text(String::from("one"))],
            vec![Value::Integer(2), Value::Text(String::from("two"))],
        ]
    );
    assert!(reloaded.as_str().contains("~A|ids|id|I2;"));
}

#[test]
fn consumed_overlay_memory_does_not_reject_a_shrinking_multi_row_update() {
    let blob = String::from(
        "V2;~S|t|id:I:!|body:T:!;~R|t|I0|Tx;~R|t|I1|Tx;~R|t|I2|Tx;~R|t|I3|Tx;~R|t|I4|Tx;~R|t|I5|Tx;~R|t|I6|Tx;~R|t|I7|Tx;",
    );
    let limits = Limits {
        max_database_bytes: 400,
        ..Limits::default()
    };
    let mut database = Database::from_string_with_limits(blob, limits)
        .expect("the source fits the configured database limit");

    assert_eq!(
        execute(&mut database, "UPDATE t SET body = ''"),
        Outcome::Affected { rows: 8 }
    );
    assert_eq!(
        rows(&mut database, "SELECT body FROM t ORDER BY id"),
        vec![vec![Value::Text(String::new())]; 8]
    );
}

#[test]
fn mutation_plan_working_state_uses_the_private_storage_budget() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE entries (value INTEGER NOT NULL)",
    );
    for value in 0..64 {
        execute(
            &mut database,
            &format!("INSERT INTO entries VALUES ({value})"),
        );
    }

    let blob = database.into_string();
    let storage_working_limit = blob.len().saturating_mul(4);
    let limits = Limits {
        max_database_bytes: blob.len(),
        ..Limits::default()
    };
    let mut limited = Database::from_string_with_limits(blob.clone(), limits)
        .expect("the source exactly fits its database limit");

    assert!(matches!(
        atomic_error(&mut limited, "UPDATE entries SET value = 0"),
        Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit,
        } if limit == storage_working_limit
    ));
    assert_eq!(limited.as_str(), blob);
}

#[test]
fn v3_constraints_and_metadata_survive_statement_wide_updates_and_rollback() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE v3_items (\
         id INTEGER PRIMARY KEY AUTOINCREMENT, \
         slug TEXT NOT NULL UNIQUE, \
         body TEXT NOT NULL DEFAULT 'seed', \
         score INTEGER NOT NULL DEFAULT 0, \
         CHECK (score >= 0), CHECK (body != ''))",
    );
    execute(&mut database, "INSERT INTO v3_items (slug) VALUES ('one')");
    execute(&mut database, "INSERT INTO v3_items (slug) VALUES ('two')");

    let before_update = database.as_str().to_owned();
    let metadata_end = before_update
        .find("~R|")
        .expect("the fixture contains row records");
    let metadata = before_update[..metadata_end].to_owned();
    assert!(metadata.starts_with("V3;"));
    assert!(metadata.contains("~A|v3_items|id|I2;"));
    assert!(metadata.contains("~D|v3_items|body|Tseed;"));
    assert!(metadata.contains("~U|v3_items|slug;"));
    assert!(metadata.contains("~C|v3_items|GE|3|I0;"));

    assert_eq!(
        execute(
            &mut database,
            "UPDATE v3_items SET body = 'a much longer replacement', score = 5",
        ),
        Outcome::Affected { rows: 2 }
    );
    assert!(database.as_str().starts_with(&metadata));
    assert_eq!(
        rows(
            &mut database,
            "SELECT id, slug, body, score FROM v3_items ORDER BY id",
        ),
        vec![
            vec![
                Value::Integer(1),
                Value::Text(String::from("one")),
                Value::Text(String::from("a much longer replacement")),
                Value::Integer(5),
            ],
            vec![
                Value::Integer(2),
                Value::Text(String::from("two")),
                Value::Text(String::from("a much longer replacement")),
                Value::Integer(5),
            ],
        ]
    );

    let before_failure = database.as_str().to_owned();
    assert!(matches!(
        atomic_error(
            &mut database,
            "UPDATE v3_items SET id = 50, slug = 'duplicate', score = -1",
        ),
        Error::Constraint(_)
    ));
    assert_eq!(database.as_str(), before_failure);

    execute(
        &mut database,
        "INSERT INTO v3_items (slug) VALUES ('three')",
    );
    assert_eq!(
        rows(
            &mut database,
            "SELECT id, body, score FROM v3_items ORDER BY id"
        ),
        vec![
            vec![
                Value::Integer(1),
                Value::Text(String::from("a much longer replacement")),
                Value::Integer(5),
            ],
            vec![
                Value::Integer(2),
                Value::Text(String::from("a much longer replacement")),
                Value::Integer(5),
            ],
            vec![
                Value::Integer(3),
                Value::Text(String::from("seed")),
                Value::Integer(0),
            ],
        ]
    );
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
