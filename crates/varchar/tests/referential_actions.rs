#![cfg(not(target_family = "wasm"))]

use varchar::{Database, Error, Limits, Outcome, Resource};

fn execute(database: &mut Database, sql: &str) -> Outcome {
    database
        .execute(sql)
        .unwrap_or_else(|error| panic!("failed to execute {sql:?}: {error}"))
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
