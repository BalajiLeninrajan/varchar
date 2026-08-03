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
fn delete_cascade_is_multi_level_reports_direct_rows_and_survives_reload() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE roots (id INTEGER PRIMARY KEY)");
    execute(
        &mut database,
        "CREATE TABLE branches (id INTEGER PRIMARY KEY, root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE)",
    );
    execute(
        &mut database,
        "CREATE TABLE leaves (id INTEGER PRIMARY KEY, branch_id INTEGER NOT NULL REFERENCES branches(id) ON DELETE CASCADE)",
    );
    for sql in [
        "INSERT INTO roots VALUES (1)",
        "INSERT INTO roots VALUES (2)",
        "INSERT INTO branches VALUES (10, 1)",
        "INSERT INTO branches VALUES (11, 1)",
        "INSERT INTO branches VALUES (20, 2)",
        "INSERT INTO leaves VALUES (100, 10)",
        "INSERT INTO leaves VALUES (101, 11)",
        "INSERT INTO leaves VALUES (200, 20)",
    ] {
        execute(&mut database, sql);
    }

    let blob = database.into_string();
    assert!(blob.starts_with("V3;"));
    assert!(blob.contains("~F|branches|root_id|roots|id|C|R;"));
    assert!(blob.contains("~F|leaves|branch_id|branches|id|C|R;"));
    let mut database = Database::from_string(blob).expect("extended action metadata reloads");

    assert_eq!(
        execute(&mut database, "DELETE FROM roots WHERE id = 1"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT id FROM roots ORDER BY id"),
        vec![vec![Value::Integer(2)]]
    );
    assert_eq!(
        rows(&mut database, "SELECT id FROM branches ORDER BY id"),
        vec![vec![Value::Integer(20)]]
    );
    assert_eq!(
        rows(&mut database, "SELECT id FROM leaves ORDER BY id"),
        vec![vec![Value::Integer(200)]]
    );

    let mut reloaded =
        Database::from_string(database.into_string()).expect("cascade result reloads");
    assert_eq!(
        rows(&mut reloaded, "SELECT id FROM leaves"),
        vec![vec![Value::Integer(200)]]
    );
}

#[test]
fn delete_set_null_preserves_children_and_update_restrict_remains_explicitly_supported() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY)",
    );
    execute(
        &mut database,
        "CREATE TABLE children (id INTEGER PRIMARY KEY, parent_id INTEGER, FOREIGN KEY (parent_id) REFERENCES parents(id) ON UPDATE RESTRICT ON DELETE SET NULL)",
    );
    execute(&mut database, "INSERT INTO parents VALUES (1)");
    execute(&mut database, "INSERT INTO parents VALUES (2)");
    execute(&mut database, "INSERT INTO children VALUES (10, 1)");
    execute(&mut database, "INSERT INTO children VALUES (11, 1)");
    execute(&mut database, "INSERT INTO children VALUES (20, 2)");

    assert!(matches!(
        atomic_error(&mut database, "UPDATE parents SET id = 3 WHERE id = 1"),
        Error::Constraint(_)
    ));
    assert_eq!(
        execute(&mut database, "DELETE FROM parents WHERE id = 1"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(
            &mut database,
            "SELECT id, parent_id FROM children ORDER BY id",
        ),
        vec![
            vec![Value::Integer(10), Value::Null],
            vec![Value::Integer(11), Value::Null],
            vec![Value::Integer(20), Value::Integer(2)],
        ]
    );

    let blob = database.into_string();
    assert!(blob.contains("~F|children|parent_id|parents|id|N|R;"));
    let mut reloaded = Database::from_string(blob).expect("SET NULL metadata reloads");
    assert_eq!(
        rows(&mut reloaded, "SELECT parent_id FROM children ORDER BY id"),
        vec![
            vec![Value::Null],
            vec![Value::Null],
            vec![Value::Integer(2)],
        ]
    );
}

#[test]
fn self_referential_cascade_handles_trees_self_loops_and_cycles() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE nodes (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES nodes(id) ON DELETE CASCADE)",
    );
    for sql in [
        "INSERT INTO nodes VALUES (1, NULL)",
        "INSERT INTO nodes VALUES (2, 1)",
        "INSERT INTO nodes VALUES (3, 2)",
        "INSERT INTO nodes VALUES (4, 4)",
        "INSERT INTO nodes VALUES (10, NULL)",
        "INSERT INTO nodes VALUES (11, 10)",
        "UPDATE nodes SET parent_id = 11 WHERE id = 10",
    ] {
        execute(&mut database, sql);
    }

    assert_eq!(
        execute(&mut database, "DELETE FROM nodes WHERE id = 1"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        execute(&mut database, "DELETE FROM nodes WHERE id = 4"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        execute(&mut database, "DELETE FROM nodes WHERE id = 10"),
        Outcome::Affected { rows: 1 }
    );
    assert!(rows(&mut database, "SELECT * FROM nodes").is_empty());
}

#[test]
fn overlapping_cascade_paths_delete_each_row_once() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE roots (id INTEGER PRIMARY KEY)");
    execute(
        &mut database,
        "CREATE TABLE left_branches (id INTEGER PRIMARY KEY, root_id INTEGER REFERENCES roots(id) ON DELETE CASCADE)",
    );
    execute(
        &mut database,
        "CREATE TABLE right_branches (id INTEGER PRIMARY KEY, root_id INTEGER REFERENCES roots(id) ON DELETE CASCADE)",
    );
    execute(
        &mut database,
        "CREATE TABLE leaves (id INTEGER PRIMARY KEY, left_id INTEGER REFERENCES left_branches(id) ON DELETE CASCADE, right_id INTEGER REFERENCES right_branches(id) ON DELETE CASCADE)",
    );
    for sql in [
        "INSERT INTO roots VALUES (1)",
        "INSERT INTO left_branches VALUES (10, 1)",
        "INSERT INTO right_branches VALUES (20, 1)",
        "INSERT INTO leaves VALUES (100, 10, 20)",
    ] {
        execute(&mut database, sql);
    }

    assert_eq!(
        execute(&mut database, "DELETE FROM roots WHERE id = 1"),
        Outcome::Affected { rows: 1 }
    );
    for table in ["roots", "left_branches", "right_branches", "leaves"] {
        assert!(
            rows(&mut database, &format!("SELECT * FROM {table}")).is_empty(),
            "{table} retained a cascaded row"
        );
    }
}

#[test]
fn a_late_restrict_failure_rolls_back_an_already_discovered_cascade_graph() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE roots (id INTEGER PRIMARY KEY)");
    execute(
        &mut database,
        "CREATE TABLE branches (id INTEGER PRIMARY KEY, root_id INTEGER REFERENCES roots(id) ON DELETE CASCADE)",
    );
    execute(
        &mut database,
        "CREATE TABLE leaves (id INTEGER PRIMARY KEY, branch_id INTEGER REFERENCES branches(id) ON DELETE CASCADE)",
    );
    execute(
        &mut database,
        "CREATE TABLE blockers (id INTEGER PRIMARY KEY, root_id INTEGER REFERENCES roots(id) ON DELETE RESTRICT)",
    );
    for sql in [
        "INSERT INTO roots VALUES (1)",
        "INSERT INTO branches VALUES (10, 1)",
        "INSERT INTO leaves VALUES (100, 10)",
        "INSERT INTO blockers VALUES (1000, 1)",
    ] {
        execute(&mut database, sql);
    }

    assert!(matches!(
        atomic_error(&mut database, "DELETE FROM roots WHERE id = 1"),
        Error::Constraint(_)
    ));
    for table in ["roots", "branches", "leaves", "blockers"] {
        assert_eq!(
            rows(&mut database, &format!("SELECT id FROM {table}")).len(),
            1,
            "{table} changed despite rollback"
        );
    }
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
fn referential_action_words_are_reserved_identifiers() {
    let mut database = Database::new();
    for (sql, keyword) in [
        ("CREATE TABLE restrict (id INTEGER PRIMARY KEY)", "RESTRICT"),
        ("CREATE TABLE t (cascade TEXT)", "CASCADE"),
    ] {
        let error = atomic_error(&mut database, sql);
        assert!(
            matches!(&error, Error::Parse { message, .. }
            if message == &format!(
                "reserved keyword `{keyword}` cannot be used as an identifier"
            )),
            "{keyword} must not be usable as an identifier, got {error}"
        );
    }

    // The words still drive the referential clauses they were reserved for.
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY)",
    );
    execute(
        &mut database,
        "CREATE TABLE children (parent_id INTEGER REFERENCES parents(id) ON DELETE SET NULL, note TEXT)",
    );
    execute(&mut database, "INSERT INTO parents VALUES (1)");
    execute(&mut database, "INSERT INTO children VALUES (1, 'kept')");

    assert_eq!(
        execute(&mut database, "DELETE FROM parents WHERE id = 1"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT parent_id, note FROM children"),
        vec![vec![Value::Null, Value::Text(String::from("kept"))]]
    );
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

#[test]
fn cascade_planning_obeys_storage_working_limits_atomically() {
    const CHILDREN: usize = 1_024;
    let mut blob =
        String::from("V3;~S|p|id:I:!;~P|p|id;~S|c|parent_id:I:!;~F|c|parent_id|p|id|C|R;~R|p|I0;");
    for _ in 0..CHILDREN {
        blob.push_str("~R|c|I0;");
    }
    let limits = Limits {
        max_database_bytes: blob.len(),
        ..Limits::default()
    };
    let mut database = Database::from_string_with_limits(blob.clone(), limits)
        .expect("the exact-size cascade fixture loads");

    assert!(matches!(
        atomic_error(&mut database, "DELETE FROM p WHERE id = 0"),
        Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            ..
        }
    ));
    assert_eq!(database.as_str(), blob);
}

#[test]
fn deletes_index_only_children_of_the_direct_parent_keys() {
    const CHILDREN: usize = 1_024;
    let mut blob = String::from(
        "V3;~S|p|id:I:!;~P|p|id;\
         ~S|c|parent_id:I:!;~F|c|parent_id|p|id|C|R;\
         ~R|p|I0;~R|p|I1;",
    );
    for _ in 0..CHILDREN {
        blob.push_str("~R|c|I0;");
    }
    let limits = Limits {
        max_database_bytes: blob.len(),
        ..Limits::default()
    };
    let mut database = Database::from_string_with_limits(blob, limits)
        .expect("the exact-size nonmatching fixture loads");

    assert_eq!(
        execute(&mut database, "DELETE FROM p WHERE id = 1"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT id FROM p"),
        vec![vec![Value::Integer(0)]]
    );
    assert_eq!(
        rows(&mut database, "SELECT parent_id FROM c").len(),
        CHILDREN
    );
}

#[test]
fn unrelated_deletes_do_not_index_the_entire_foreign_key_database() {
    const CHILDREN: usize = 1_024;
    let mut blob = String::from(
        "V3;~S|p|id:I:!;~P|p|id;\
         ~S|c|parent_id:I:!;~F|c|parent_id|p|id|C|R;\
         ~S|unrelated|id:I:!;~P|unrelated|id;\
         ~R|p|I0;~R|unrelated|I1;",
    );
    for _ in 0..CHILDREN {
        blob.push_str("~R|c|I0;");
    }
    let limits = Limits {
        max_database_bytes: blob.len(),
        ..Limits::default()
    };
    let mut database = Database::from_string_with_limits(blob, limits)
        .expect("the exact-size unrelated fixture loads");

    assert_eq!(
        execute(&mut database, "DELETE FROM unrelated WHERE id = 1"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT id FROM p"),
        vec![vec![Value::Integer(0)]]
    );
    assert_eq!(
        rows(&mut database, "SELECT parent_id FROM c").len(),
        CHILDREN
    );
}
