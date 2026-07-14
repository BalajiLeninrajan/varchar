#![cfg(not(target_family = "wasm"))]

use varchar::{DataType, Database, Error, ErrorCode, Outcome, RowSet, Value};

fn execute(database: &mut Database, sql: &str) -> Outcome {
    database
        .execute(sql)
        .unwrap_or_else(|error| panic!("failed to execute {sql:?}: {error}"))
}

fn rows(outcome: Outcome) -> RowSet {
    match outcome {
        Outcome::Rows(rows) => rows,
        other => panic!("expected rows, got {other:?}"),
    }
}

fn expect_atomic_error(database: &mut Database, sql: &str) -> Error {
    let before = database.as_str().to_owned();
    let error = match database.execute(sql) {
        Ok(outcome) => panic!("unexpectedly accepted {sql:?}: {outcome:?}"),
        Err(error) => error,
    };
    assert_eq!(
        database.as_str(),
        before,
        "failed mutation changed the database: {sql:?}"
    );
    error
}

#[test]
fn inline_and_table_primary_keys_are_non_null_and_unique() {
    let mut database = Database::new();

    execute(
        &mut database,
        "CREATE TABLE inline_keys (id INTEGER PRIMARY KEY, note TEXT)",
    );
    let inline_columns = rows(execute(&mut database, "SELECT * FROM inline_keys"))
        .columns()
        .to_vec();
    assert_eq!(inline_columns[0].label(), "id");
    assert_eq!(inline_columns[0].origin().table(), "inline_keys");
    assert_eq!(inline_columns[0].origin().column(), "id");
    assert_eq!(inline_columns[0].data_type(), DataType::Integer);
    assert!(
        !inline_columns[0].nullable(),
        "a primary key implies NOT NULL"
    );

    execute(&mut database, "INSERT INTO inline_keys VALUES (1, 'kept')");
    assert_eq!(
        expect_atomic_error(
            &mut database,
            "INSERT INTO inline_keys VALUES (1, 'duplicate')",
        )
        .code(),
        ErrorCode::Constraint
    );
    assert_eq!(
        expect_atomic_error(
            &mut database,
            "INSERT INTO inline_keys VALUES (NULL, 'null key')",
        )
        .code(),
        ErrorCode::Type
    );

    execute(
        &mut database,
        "CREATE TABLE table_keys (id INTEGER, note TEXT, PRIMARY KEY (id))",
    );
    let table_columns = rows(execute(&mut database, "SELECT * FROM table_keys"))
        .columns()
        .to_vec();
    assert_eq!(table_columns[0].label(), "id");
    assert_eq!(table_columns[0].origin().table(), "table_keys");
    assert_eq!(table_columns[0].origin().column(), "id");
    assert_eq!(table_columns[0].data_type(), DataType::Integer);
    assert!(
        !table_columns[0].nullable(),
        "a table-level primary key implies NOT NULL"
    );

    execute(&mut database, "INSERT INTO table_keys VALUES (7, 'kept')");
    expect_atomic_error(
        &mut database,
        "INSERT INTO table_keys VALUES (7, 'duplicate')",
    );
    expect_atomic_error(
        &mut database,
        "INSERT INTO table_keys VALUES (NULL, 'null key')",
    );
}

#[test]
fn inline_constraints_accept_mixed_modifier_order() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY)",
    );
    execute(
        &mut database,
        "CREATE TABLE owners (id INTEGER PRIMARY KEY)",
    );
    execute(&mut database, "INSERT INTO parents VALUES (1)");
    execute(&mut database, "INSERT INTO parents VALUES (2)");
    execute(&mut database, "INSERT INTO owners VALUES (7)");

    execute(
        &mut database,
        "CREATE TABLE children (id INTEGER REFERENCES parents(id) PRIMARY KEY, owner_id INTEGER NOT NULL REFERENCES owners(id), note TEXT)",
    );

    let columns = rows(execute(&mut database, "SELECT * FROM children"))
        .columns()
        .to_vec();
    for (column, name) in columns.iter().zip(["id", "owner_id", "note"]) {
        assert_eq!(column.origin().table(), "children");
        assert_eq!(column.origin().column(), name);
    }
    assert!(!columns[0].nullable(), "a primary key implies NOT NULL");
    assert!(!columns[1].nullable(), "an explicit NOT NULL is preserved");
    assert!(columns[2].nullable());

    execute(&mut database, "INSERT INTO children VALUES (1, 7, 'kept')");
    for sql in [
        "INSERT INTO children VALUES (1, 7, 'duplicate key')",
        "INSERT INTO children VALUES (99, 7, 'missing parent')",
        "INSERT INTO children VALUES (2, 99, 'missing owner')",
    ] {
        assert_eq!(
            expect_atomic_error(&mut database, sql).code(),
            ErrorCode::Constraint
        );
    }
}

#[test]
fn primary_key_update_collisions_fail_atomically() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE accounts (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
    );
    execute(&mut database, "INSERT INTO accounts VALUES (1, 'Ada')");
    execute(&mut database, "INSERT INTO accounts VALUES (2, 'Grace')");

    expect_atomic_error(&mut database, "UPDATE accounts SET id = 1 WHERE id = 2");
    expect_atomic_error(&mut database, "UPDATE accounts SET id = 3");
    expect_atomic_error(&mut database, "UPDATE accounts SET id = NULL WHERE id = 2");

    assert_eq!(
        rows(execute(&mut database, "SELECT id, name FROM accounts")).rows(),
        vec![
            vec![Value::Integer(1), Value::Text("Ada".to_owned())],
            vec![Value::Integer(2), Value::Text("Grace".to_owned())],
        ]
    );
}

#[test]
fn invalid_key_definitions_are_rejected_without_mutating() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY, code INTEGER, label TEXT)",
    );

    for sql in [
        "CREATE TABLE two_keys (a INTEGER PRIMARY KEY, b INTEGER PRIMARY KEY)",
        "CREATE TABLE duplicate_columns (id INTEGER, id INTEGER)",
        "CREATE TABLE duplicate_not_null (id INTEGER NOT NULL NOT NULL)",
        "CREATE TABLE duplicate_inline_key (id INTEGER PRIMARY KEY PRIMARY KEY)",
        "CREATE TABLE missing_table (parent_id INTEGER REFERENCES absent(id))",
        "CREATE TABLE missing_column (parent_id INTEGER REFERENCES parents(absent))",
        "CREATE TABLE non_key_target (parent_code INTEGER REFERENCES parents(code))",
        "CREATE TABLE wrong_type (parent_id TEXT REFERENCES parents(id))",
        "CREATE TABLE missing_local (id INTEGER, PRIMARY KEY (absent))",
        "CREATE TABLE duplicate_key (id INTEGER PRIMARY KEY, PRIMARY KEY (id))",
        "CREATE TABLE duplicate_inline_reference (parent_id INTEGER REFERENCES parents(id) REFERENCES parents(id))",
        "CREATE TABLE duplicate_reference (id INTEGER, parent_id INTEGER REFERENCES parents(id), FOREIGN KEY (parent_id) REFERENCES parents(id))",
    ] {
        assert_eq!(
            expect_atomic_error(&mut database, sql).code(),
            ErrorCode::Schema,
            "expected a schema error for {sql:?}"
        );
    }
}

#[test]
fn composite_constraints_are_explicitly_unsupported_and_atomic() {
    let mut database = Database::new();

    for sql in [
        "CREATE TABLE t (a INTEGER, b INTEGER, PRIMARY KEY (a, b))",
        "CREATE TABLE t (a INTEGER, b INTEGER, FOREIGN KEY (a, b) REFERENCES p(a))",
        "CREATE TABLE t (a INTEGER REFERENCES p(a, b))",
    ] {
        assert_eq!(
            expect_atomic_error(&mut database, sql).code(),
            ErrorCode::UnsupportedSql,
            "expected composite constraint to be unsupported: {sql}"
        );
    }
}

#[test]
fn inline_and_table_foreign_keys_accept_valid_and_null_values_but_reject_orphans() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE inline_children (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parents(id))",
    );
    execute(
        &mut database,
        "CREATE TABLE table_children (id INTEGER PRIMARY KEY, parent_id INTEGER, FOREIGN KEY (parent_id) REFERENCES parents(id))",
    );
    execute(&mut database, "INSERT INTO parents VALUES (1, 'parent')");

    for sql in [
        "INSERT INTO inline_children VALUES (10, 1)",
        "INSERT INTO inline_children VALUES (11, NULL)",
        "INSERT INTO table_children VALUES (20, 1)",
        "INSERT INTO table_children VALUES (21, NULL)",
    ] {
        execute(&mut database, sql);
    }

    assert_eq!(
        expect_atomic_error(
            &mut database,
            "INSERT INTO inline_children VALUES (12, 999)",
        )
        .code(),
        ErrorCode::Constraint
    );
    assert_eq!(
        expect_atomic_error(&mut database, "INSERT INTO table_children VALUES (22, 999)").code(),
        ErrorCode::Constraint
    );
    assert_eq!(
        expect_atomic_error(
            &mut database,
            "UPDATE inline_children SET parent_id = 999 WHERE id = 10",
        )
        .code(),
        ErrorCode::Constraint
    );

    assert_eq!(
        rows(execute(
            &mut database,
            "SELECT id, parent_id FROM inline_children",
        ))
        .rows(),
        vec![
            vec![Value::Integer(10), Value::Integer(1)],
            vec![Value::Integer(11), Value::Null],
        ]
    );
    assert_eq!(
        rows(execute(
            &mut database,
            "SELECT id, parent_id FROM table_children",
        ))
        .rows(),
        vec![
            vec![Value::Integer(20), Value::Integer(1)],
            vec![Value::Integer(21), Value::Null],
        ]
    );
}

#[test]
fn foreign_keys_restrict_parent_updates_and_deletes_until_children_are_removed() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE children (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parents(id))",
    );
    execute(&mut database, "INSERT INTO parents VALUES (1, 'parent')");
    execute(&mut database, "INSERT INTO children VALUES (10, 1)");

    expect_atomic_error(&mut database, "UPDATE parents SET id = 2 WHERE id = 1");
    expect_atomic_error(&mut database, "DELETE FROM parents WHERE id = 1");

    assert_eq!(
        execute(&mut database, "DELETE FROM children WHERE id = 10"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        execute(&mut database, "DELETE FROM parents WHERE id = 1"),
        Outcome::Affected { rows: 1 }
    );
    assert!(
        rows(execute(&mut database, "SELECT * FROM parents"))
            .rows()
            .is_empty()
    );
}

#[test]
fn self_referential_foreign_keys_are_checked_against_the_candidate_database() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE nodes (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES nodes(id))",
    );
    execute(&mut database, "INSERT INTO nodes VALUES (1, NULL)");
    execute(&mut database, "INSERT INTO nodes VALUES (2, 1)");
    execute(&mut database, "INSERT INTO nodes VALUES (3, 3)");

    expect_atomic_error(&mut database, "DELETE FROM nodes WHERE id = 1");
    expect_atomic_error(&mut database, "UPDATE nodes SET id = 4 WHERE id = 3");
    expect_atomic_error(&mut database, "INSERT INTO nodes VALUES (5, 999)");

    execute(&mut database, "DELETE FROM nodes WHERE id = 2");
    execute(&mut database, "DELETE FROM nodes WHERE id = 1");
    execute(
        &mut database,
        "UPDATE nodes SET id = 4, parent_id = 4 WHERE id = 3",
    );
    assert_eq!(
        rows(execute(&mut database, "SELECT id, parent_id FROM nodes")).rows(),
        vec![vec![Value::Integer(4), Value::Integer(4)]]
    );
    assert_eq!(
        execute(&mut database, "DELETE FROM nodes WHERE id = 4"),
        Outcome::Affected { rows: 1 }
    );
}

#[test]
fn self_referential_cycles_require_a_coordinated_delete() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE nodes (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES nodes(id))",
    );
    execute(&mut database, "INSERT INTO nodes VALUES (1, NULL)");
    execute(&mut database, "INSERT INTO nodes VALUES (2, 1)");
    execute(&mut database, "UPDATE nodes SET parent_id = 2 WHERE id = 1");

    assert_eq!(
        expect_atomic_error(&mut database, "DELETE FROM nodes WHERE id = 1").code(),
        ErrorCode::Constraint
    );
    assert_eq!(
        expect_atomic_error(&mut database, "DELETE FROM nodes WHERE id = 2").code(),
        ErrorCode::Constraint
    );
    assert_eq!(
        execute(&mut database, "DELETE FROM nodes"),
        Outcome::Affected { rows: 2 }
    );
}

#[test]
fn primary_and_foreign_key_constraints_survive_reload() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER, PRIMARY KEY (id))",
    );
    execute(
        &mut database,
        "CREATE TABLE children (id INTEGER PRIMARY KEY, parent_id INTEGER, FOREIGN KEY (parent_id) REFERENCES parents(id))",
    );
    execute(&mut database, "INSERT INTO parents VALUES (1)");
    execute(&mut database, "INSERT INTO children VALUES (10, 1)");

    let blob = database.into_string();
    let mut reloaded = Database::from_string(blob.clone()).expect("constrained blob reloads");
    assert_eq!(reloaded.as_str(), blob);

    expect_atomic_error(&mut reloaded, "INSERT INTO parents VALUES (1)");
    expect_atomic_error(&mut reloaded, "INSERT INTO children VALUES (11, 999)");
    expect_atomic_error(&mut reloaded, "DELETE FROM parents WHERE id = 1");

    assert_eq!(
        rows(execute(&mut reloaded, "SELECT * FROM children")).rows(),
        vec![vec![Value::Integer(10), Value::Integer(1)]]
    );
}

#[test]
fn constrained_blobs_with_duplicate_keys_or_orphan_references_are_rejected() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY)",
    );
    execute(
        &mut database,
        "CREATE TABLE children (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parents(id))",
    );
    execute(&mut database, "INSERT INTO parents VALUES (1)");
    execute(&mut database, "INSERT INTO parents VALUES (2)");
    execute(&mut database, "INSERT INTO children VALUES (10, 1)");
    let blob = database.into_string();

    let duplicate_key = blob.replacen("~R|parents|I2;", "~R|parents|I1;", 1);
    assert_ne!(
        duplicate_key, blob,
        "parent row encoding changed unexpectedly"
    );
    assert!(matches!(
        Database::from_string(duplicate_key),
        Err(error) if error.code() == ErrorCode::CorruptStorage
    ));

    let orphan = blob.replacen("~R|children|I10|I1;", "~R|children|I10|I9;", 1);
    assert_ne!(orphan, blob, "child row encoding changed unexpectedly");
    assert!(matches!(
        Database::from_string(orphan),
        Err(error) if error.code() == ErrorCode::CorruptStorage
    ));
}

#[test]
fn constraint_words_remain_available_as_contextual_identifiers() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE words (key TEXT, primary TEXT, foreign TEXT, references TEXT)",
    );
    execute(
        &mut database,
        "INSERT INTO words VALUES ('k', 'p', 'f', 'r')",
    );

    assert_eq!(
        rows(execute(
            &mut database,
            "SELECT key, primary, foreign, references FROM words",
        ))
        .rows(),
        vec![vec![
            Value::Text("k".to_owned()),
            Value::Text("p".to_owned()),
            Value::Text("f".to_owned()),
            Value::Text("r".to_owned()),
        ]]
    );
}
