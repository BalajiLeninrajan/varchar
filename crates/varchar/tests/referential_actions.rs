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
    assert_eq!(database.as_str(), before, "failed statement changed state");
    error
}

#[test]
fn default_and_explicit_restrict_block_parent_mutations_atomically() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY)",
    );
    execute(
        &mut database,
        "CREATE TABLE default_children (parent_id INTEGER REFERENCES parents(id))",
    );
    execute(
        &mut database,
        "CREATE TABLE explicit_children (parent_id INTEGER REFERENCES parents(id) ON DELETE RESTRICT ON UPDATE RESTRICT)",
    );
    execute(&mut database, "INSERT INTO parents VALUES (1)");
    execute(&mut database, "INSERT INTO parents VALUES (2)");
    execute(&mut database, "INSERT INTO default_children VALUES (1)");
    execute(&mut database, "INSERT INTO explicit_children VALUES (2)");

    for sql in [
        "DELETE FROM parents WHERE id = 1",
        "UPDATE parents SET id = 10 WHERE id = 1",
        "DELETE FROM parents WHERE id = 2",
        "UPDATE parents SET id = 20 WHERE id = 2",
    ] {
        assert!(matches!(
            atomic_error(&mut database, sql),
            Error::Constraint(_)
        ));
    }

    assert!(
        database
            .as_str()
            .contains("~F|default_children|parent_id|parents|id;")
    );
    assert!(
        database
            .as_str()
            .contains("~F|explicit_children|parent_id|parents|id;")
    );
    assert!(!database.as_str().contains("|R|R;"));
    assert!(database.as_str().starts_with("V2;"));
}

#[test]
fn restrict_admits_coordinated_self_referential_mutations() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE nodes (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES nodes(id))",
    );
    execute(&mut database, "INSERT INTO nodes VALUES (1, 1)");

    // The reference is rewritten by the same statement that re-keys its parent,
    // so the candidate database never holds a dangling row.
    assert_eq!(
        execute(
            &mut database,
            "UPDATE nodes SET id = 2, parent_id = 2 WHERE id = 1",
        ),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT id, parent_id FROM nodes"),
        vec![vec![Value::Integer(2), Value::Integer(2)]]
    );

    // A rewrite that lands on a key nothing supplies still dangles, and the
    // candidate-side check rejects it.
    assert!(matches!(
        atomic_error(
            &mut database,
            "UPDATE nodes SET id = 3, parent_id = 9 WHERE id = 2",
        ),
        Error::Constraint(_)
    ));
    // Re-keying without moving the reference dangles just as plainly.
    assert!(matches!(
        atomic_error(&mut database, "UPDATE nodes SET id = 3 WHERE id = 2"),
        Error::Constraint(_)
    ));

    assert_eq!(
        execute(&mut database, "DELETE FROM nodes"),
        Outcome::Affected { rows: 1 }
    );
}

#[test]
fn restrict_still_rejects_mutations_that_strand_an_uninvolved_child() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE nodes (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES nodes(id))",
    );
    execute(&mut database, "INSERT INTO nodes VALUES (1, NULL)");
    execute(&mut database, "INSERT INTO nodes VALUES (2, 1)");

    // Row 2 is outside both statements' target sets, so its reference to row 1
    // is exactly what RESTRICT exists to protect.
    for sql in [
        "UPDATE nodes SET id = 3 WHERE id = 1",
        "UPDATE nodes SET id = 3, parent_id = 3 WHERE id = 1",
        "DELETE FROM nodes WHERE id = 1",
    ] {
        assert!(
            matches!(atomic_error(&mut database, sql), Error::Constraint(_)),
            "{sql:?} must remain restricted"
        );
    }

    // Naming both rows lets the same statement move the parent and its child.
    assert_eq!(
        execute(&mut database, "DELETE FROM nodes"),
        Outcome::Affected { rows: 2 }
    );
}

#[test]
fn v2_rejects_extended_foreign_key_action_metadata() {
    let blob = "V2;~S|parents|id:I:!;~P|parents|id;\
                ~S|children|parent_id:I:?;\
                ~F|children|parent_id|parents|id|C|R;";
    let offset = blob.find("~F|").expect("foreign key exists");
    assert!(matches!(
        Database::from_string(String::from(blob)),
        Err(Error::CorruptStorage { offset: actual, message })
            if actual == offset && message == "V3 metadata is invalid under a V2 header"
    ));
}

#[test]
fn unsupported_on_update_cascade_is_typed_and_atomic() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY)",
    );

    assert!(matches!(
        atomic_error(
            &mut database,
            "CREATE TABLE children (parent_id INTEGER REFERENCES parents(id) ON UPDATE CASCADE)",
        ),
        Error::Unsupported { ref feature, .. } if feature == "ON UPDATE CASCADE"
    ));
}

#[test]
fn set_null_nullability_precedes_later_declaration_errors() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY)",
    );

    assert!(matches!(
        atomic_error(
            &mut database,
            "CREATE TABLE children (parent_id INTEGER NOT NULL REFERENCES parents(id) ON DELETE SET NULL, value INTEGER UNIQUE UNIQUE)",
        ),
        Error::Schema(ref message)
            if message
                == "ON DELETE SET NULL requires nullable foreign-key column \"children\".\"parent_id\""
    ));
}

#[test]
fn action_metadata_obeys_database_byte_limits_atomically() {
    let mut reference = Database::new();
    execute(
        &mut reference,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY)",
    );
    let parent_blob = reference.as_str().to_owned();
    execute(
        &mut reference,
        "CREATE TABLE children (parent_id INTEGER REFERENCES parents(id) ON DELETE CASCADE)",
    );
    assert!(parent_blob.starts_with("V2;"));
    assert!(reference.as_str().starts_with("V3;"));
    let required = reference.as_str().len();

    let limits = Limits {
        max_database_bytes: required - 1,
        ..Limits::default()
    };
    let mut limited = Database::from_string_with_limits(parent_blob.clone(), limits)
        .expect("the parent-only database fits");
    assert!(matches!(
        atomic_error(
            &mut limited,
            "CREATE TABLE children (parent_id INTEGER REFERENCES parents(id) ON DELETE CASCADE)",
        ),
        Error::ResourceLimit {
            resource: Resource::DatabaseBytes,
            limit,
        } if limit == required - 1
    ));
    assert_eq!(limited.as_str(), parent_blob);
}
