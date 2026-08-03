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
fn unchanged_parent_keys_skip_update_restrict_checks() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY, note TEXT NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE children (parent_id INTEGER REFERENCES parents(id) ON UPDATE RESTRICT)",
    );
    execute(&mut database, "INSERT INTO parents VALUES (1, 'old')");
    execute(&mut database, "INSERT INTO children VALUES (1)");

    assert_eq!(
        execute(
            &mut database,
            "UPDATE parents SET id = 1, note = 'new' WHERE id = 1",
        ),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT id, note FROM parents"),
        vec![vec![Value::Integer(1), Value::Text(String::from("new")),]]
    );
    assert_eq!(
        rows(&mut database, "SELECT parent_id FROM children"),
        vec![vec![Value::Integer(1)]]
    );
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
fn update_cascade_is_multi_level_reports_direct_rows_and_survives_reload() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE roots (id INTEGER PRIMARY KEY)");
    execute(
        &mut database,
        "CREATE TABLE branches (id INTEGER PRIMARY KEY REFERENCES roots(id) ON UPDATE CASCADE)",
    );
    execute(
        &mut database,
        "CREATE TABLE leaves (id INTEGER PRIMARY KEY, branch_id INTEGER REFERENCES branches(id) ON UPDATE CASCADE)",
    );
    for sql in [
        "INSERT INTO roots VALUES (1)",
        "INSERT INTO roots VALUES (2)",
        "INSERT INTO branches VALUES (1)",
        "INSERT INTO branches VALUES (2)",
        "INSERT INTO leaves VALUES (100, 1)",
        "INSERT INTO leaves VALUES (200, 2)",
    ] {
        execute(&mut database, sql);
    }

    let blob = database.into_string();
    assert!(blob.starts_with("V3;"));
    assert!(blob.contains("~F|branches|id|roots|id|R|C;"));
    assert!(blob.contains("~F|leaves|branch_id|branches|id|R|C;"));
    let mut database = Database::from_string(blob).expect("update actions reload");

    assert_eq!(
        execute(&mut database, "UPDATE roots SET id = 10 WHERE id = 1"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT id FROM roots ORDER BY id"),
        vec![vec![Value::Integer(2)], vec![Value::Integer(10)]]
    );
    assert_eq!(
        rows(&mut database, "SELECT id FROM branches ORDER BY id"),
        vec![vec![Value::Integer(2)], vec![Value::Integer(10)]]
    );
    assert_eq!(
        rows(
            &mut database,
            "SELECT id, branch_id FROM leaves ORDER BY id",
        ),
        vec![
            vec![Value::Integer(100), Value::Integer(10)],
            vec![Value::Integer(200), Value::Integer(2)],
        ]
    );

    let mut reloaded =
        Database::from_string(database.into_string()).expect("cascaded updates reload");
    assert_eq!(
        rows(&mut reloaded, "SELECT branch_id FROM leaves ORDER BY id"),
        vec![vec![Value::Integer(10)], vec![Value::Integer(2)]]
    );
}

#[test]
fn self_referential_update_cascade_merges_direct_and_induced_changes() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE nodes (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES nodes(id) ON UPDATE CASCADE)",
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
        execute(&mut database, "UPDATE nodes SET id = 20 WHERE id = 1"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT parent_id FROM nodes WHERE id = 2"),
        vec![vec![Value::Integer(20)]]
    );
    assert_eq!(
        execute(&mut database, "UPDATE nodes SET id = 40 WHERE id = 4"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(
            &mut database,
            "SELECT id, parent_id FROM nodes WHERE id = 40",
        ),
        vec![vec![Value::Integer(40), Value::Integer(40)]]
    );
    assert_eq!(
        execute(&mut database, "UPDATE nodes SET id = 100 WHERE id = 10"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(
            &mut database,
            "SELECT id, parent_id FROM nodes WHERE id = 11 OR id = 100 ORDER BY id",
        ),
        vec![
            vec![Value::Integer(11), Value::Integer(100)],
            vec![Value::Integer(100), Value::Integer(11)],
        ]
    );
}

#[test]
fn text_update_cascade_accounts_payloads_at_exact_working_boundaries() {
    const CHILDREN: usize = 1_024;
    let mut source = String::from(
        "V3;~S|p|id:T:!;~P|p|id;\
         ~S|c|parent_id:T:!;~F|c|parent_id|p|id|R|C;\
         ~R|p|Ta;",
    );
    for _ in 0..CHILDREN {
        source.push_str("~R|c|Ta;");
    }
    let sql = "UPDATE p SET id = 'bb' WHERE id = 'a'";
    let mut probe = Database::from_string(source.clone()).expect("TEXT cascade fixture loads");
    execute(&mut probe, sql);
    let required_database_bytes = probe.as_str().len();

    let mut lower = required_database_bytes;
    let mut upper = required_database_bytes.saturating_mul(8);
    loop {
        let mut candidate = Database::from_string_with_limits(
            source.clone(),
            Limits {
                max_database_bytes: upper,
                ..Limits::default()
            },
        )
        .expect("the source fits the searched upper bound");
        if candidate.execute(sql).is_ok() {
            break;
        }
        upper = upper
            .checked_mul(2)
            .expect("the TEXT cascade working boundary fits usize");
    }
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        let mut candidate = Database::from_string_with_limits(
            source.clone(),
            Limits {
                max_database_bytes: middle,
                ..Limits::default()
            },
        )
        .expect("the source fits the searched limit");
        if candidate.execute(sql).is_ok() {
            upper = middle;
        } else {
            lower = middle + 1;
        }
    }
    let exact = lower;
    assert!(
        exact > required_database_bytes,
        "TEXT cascade working state exceeds its candidate-size boundary"
    );

    let mut exact_database = Database::from_string_with_limits(
        source.clone(),
        Limits {
            max_database_bytes: exact,
            ..Limits::default()
        },
    )
    .expect("the source fits the exact working boundary");
    assert_eq!(
        execute(&mut exact_database, sql),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut exact_database, "SELECT parent_id FROM c").len(),
        CHILDREN
    );

    let one_under = exact - 1;
    let mut limited = Database::from_string_with_limits(
        source.clone(),
        Limits {
            max_database_bytes: one_under,
            ..Limits::default()
        },
    )
    .expect("the source fits the one-under working boundary");
    assert!(matches!(
        atomic_error(&mut limited, sql),
        Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            ..
        }
    ));
    assert_eq!(limited.as_str(), source);
}

#[test]
fn overlapping_update_cascade_paths_merge_each_row_once() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE roots (id INTEGER PRIMARY KEY)");
    execute(
        &mut database,
        "CREATE TABLE left_branches (id INTEGER PRIMARY KEY REFERENCES roots(id) ON UPDATE CASCADE)",
    );
    execute(
        &mut database,
        "CREATE TABLE right_branches (id INTEGER PRIMARY KEY REFERENCES roots(id) ON UPDATE CASCADE)",
    );
    execute(
        &mut database,
        "CREATE TABLE leaves (id INTEGER PRIMARY KEY, left_id INTEGER REFERENCES left_branches(id) ON UPDATE CASCADE, right_id INTEGER REFERENCES right_branches(id) ON UPDATE CASCADE, note TEXT NOT NULL)",
    );
    for sql in [
        "INSERT INTO roots VALUES (1)",
        "INSERT INTO left_branches VALUES (1)",
        "INSERT INTO right_branches VALUES (1)",
        "INSERT INTO leaves VALUES (100, 1, 1, 'kept')",
    ] {
        execute(&mut database, sql);
    }

    assert_eq!(
        execute(&mut database, "UPDATE roots SET id = 9 WHERE id = 1"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT left_id, right_id, note FROM leaves"),
        vec![vec![
            Value::Integer(9),
            Value::Integer(9),
            Value::Text(String::from("kept")),
        ]]
    );
}

#[test]
fn update_cascade_conflicts_and_late_restrict_failures_are_atomic() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE roots (id INTEGER PRIMARY KEY)");
    execute(
        &mut database,
        "CREATE TABLE branches (id INTEGER PRIMARY KEY REFERENCES roots(id) ON UPDATE CASCADE)",
    );
    execute(
        &mut database,
        "CREATE TABLE leaves (branch_id INTEGER REFERENCES branches(id) ON UPDATE RESTRICT)",
    );
    for sql in [
        "INSERT INTO roots VALUES (1)",
        "INSERT INTO branches VALUES (1)",
        "INSERT INTO leaves VALUES (1)",
    ] {
        execute(&mut database, sql);
    }
    assert!(matches!(
        atomic_error(&mut database, "UPDATE roots SET id = 10 WHERE id = 1"),
        Error::Constraint(_)
    ));

    let mut self_reference = Database::new();
    execute(
        &mut self_reference,
        "CREATE TABLE nodes (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES nodes(id) ON UPDATE CASCADE)",
    );
    execute(&mut self_reference, "INSERT INTO nodes VALUES (1, 1)");
    execute(&mut self_reference, "INSERT INTO nodes VALUES (3, NULL)");
    assert!(matches!(
        atomic_error(
            &mut self_reference,
            "UPDATE nodes SET id = 2, parent_id = 3 WHERE id = 1",
        ),
        Error::Constraint(_)
    ));
    assert_eq!(
        execute(
            &mut self_reference,
            "UPDATE nodes SET id = 2, parent_id = 2 WHERE id = 1",
        ),
        Outcome::Affected { rows: 1 }
    );
}

#[test]
fn restricted_references_stay_coordinated_while_cascades_expand() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE nodes (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES nodes(id) ON UPDATE RESTRICT)",
    );
    execute(
        &mut database,
        "CREATE TABLE tags (node_id INTEGER REFERENCES nodes(id) ON UPDATE CASCADE)",
    );
    execute(&mut database, "INSERT INTO nodes VALUES (3, 3)");
    execute(&mut database, "INSERT INTO tags VALUES (3)");

    // The restricted self-reference is rewritten by the statement that re-keys
    // it, so it releases the restriction even while the same key expansion
    // cascades into another table.
    assert_eq!(
        execute(
            &mut database,
            "UPDATE nodes SET id = 4, parent_id = 4 WHERE id = 3",
        ),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT id, parent_id FROM nodes"),
        vec![vec![Value::Integer(4), Value::Integer(4)]]
    );
    assert_eq!(
        rows(&mut database, "SELECT node_id FROM tags"),
        vec![vec![Value::Integer(4)]]
    );

    // A restricted row the statement never names keeps holding the old key, so
    // the cascade cannot excuse it.
    execute(&mut database, "INSERT INTO nodes VALUES (5, 4)");
    assert!(matches!(
        atomic_error(
            &mut database,
            "UPDATE nodes SET id = 6, parent_id = 6 WHERE id = 4",
        ),
        Error::Constraint(_)
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
fn update_cascade_obeys_database_and_storage_working_limits_atomically() {
    let mut reference = Database::new();
    execute(&mut reference, "CREATE TABLE p (id INTEGER PRIMARY KEY)");
    execute(
        &mut reference,
        "CREATE TABLE c (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES p(id) ON UPDATE CASCADE)",
    );
    execute(&mut reference, "CREATE TABLE padding (value TEXT NOT NULL)");
    execute(
        &mut reference,
        &format!("INSERT INTO padding VALUES ('{}')", "x".repeat(1_024)),
    );
    execute(&mut reference, "INSERT INTO p VALUES (1)");
    for id in 0..1 {
        execute(&mut reference, &format!("INSERT INTO c VALUES ({id}, 1)"));
    }
    let source = reference.into_string();
    let mut probe = Database::from_string(source.clone()).expect("probe source reloads");
    execute(&mut probe, "UPDATE p SET id = 100000 WHERE id = 1");
    let required = probe.as_str().len();

    let mut exact = Database::from_string_with_limits(
        source.clone(),
        Limits {
            max_database_bytes: required,
            ..Limits::default()
        },
    )
    .expect("source fits exact result limit");
    execute(&mut exact, "UPDATE p SET id = 100000 WHERE id = 1");
    assert_eq!(exact.as_str().len(), required);

    let limit = required - 1;
    let mut limited = Database::from_string_with_limits(
        source.clone(),
        Limits {
            max_database_bytes: limit,
            ..Limits::default()
        },
    )
    .expect("source fits one-under result limit");
    assert!(matches!(
        atomic_error(&mut limited, "UPDATE p SET id = 100000 WHERE id = 1"),
        Error::ResourceLimit {
            resource: Resource::DatabaseBytes,
            limit: actual,
        } if actual == limit
    ));
    assert_eq!(limited.as_str(), source);

    const CHILDREN: usize = 1_024;
    let mut dense =
        String::from("V3;~S|p|id:I:!;~P|p|id;~S|c|parent_id:I:!;~F|c|parent_id|p|id|R|C;~R|p|I0;");
    for _ in 0..CHILDREN {
        dense.push_str("~R|c|I0;");
    }
    let mut dense_database = Database::from_string_with_limits(
        dense.clone(),
        Limits {
            max_database_bytes: dense.len(),
            ..Limits::default()
        },
    )
    .expect("dense update-cascade fixture loads");
    assert!(matches!(
        atomic_error(&mut dense_database, "UPDATE p SET id = 1 WHERE id = 0"),
        Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            ..
        }
    ));
    assert_eq!(dense_database.as_str(), dense);
}

#[test]
fn non_primary_key_cascades_do_not_index_unreachable_descendants() {
    const GRANDCHILDREN: usize = 1_024;
    let mut blob = String::from(
        "V3;~S|p|id:I:!;~P|p|id;\
         ~S|c|id:I:!|parent_id:I:!;~P|c|id;~F|c|parent_id|p|id|R|C;\
         ~S|g|c_id:I:!;~F|g|c_id|c|id|R|C;\
         ~R|p|I0;~R|c|I1|I0;",
    );
    for _ in 0..GRANDCHILDREN {
        blob.push_str("~R|g|I1;");
    }
    let limits = Limits {
        max_database_bytes: blob.len(),
        ..Limits::default()
    };
    let mut database = Database::from_string_with_limits(blob, limits)
        .expect("the exact-size non-propagating cascade fixture loads");

    assert_eq!(
        execute(&mut database, "UPDATE p SET id = 2 WHERE id = 0"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT id, parent_id FROM c"),
        vec![vec![Value::Integer(1), Value::Integer(2)]]
    );
    assert_eq!(
        rows(&mut database, "SELECT c_id FROM g").len(),
        GRANDCHILDREN
    );
}

#[test]
fn updates_index_only_children_of_the_direct_parent_keys() {
    const CHILDREN: usize = 1_024;
    let mut blob = String::from(
        "V3;~S|p|id:I:!;~P|p|id;\
         ~S|c|parent_id:I:!;~F|c|parent_id|p|id|R|C;\
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
        .expect("the exact-size nonmatching update fixture loads");

    assert_eq!(
        execute(&mut database, "UPDATE p SET id = 2 WHERE id = 1"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT id FROM p ORDER BY id"),
        vec![vec![Value::Integer(0)], vec![Value::Integer(2)]]
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
