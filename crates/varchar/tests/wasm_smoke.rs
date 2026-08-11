#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

use varchar::{Database, Error, Limits, Outcome, Resource, RowSet, SelectExplanation, Value};
use wasm_bindgen_test::wasm_bindgen_test;

fn rows(outcome: Outcome) -> Vec<Vec<Value>> {
    match outcome {
        Outcome::Rows(row_set) => row_set.into_rows(),
        other => panic!("expected rows, got {other:?}"),
    }
}

#[wasm_bindgen_test]
fn typed_crud_and_regex_planning_execute_inside_wasm() {
    let mut database = Database::new();
    assert_eq!(
        database
            .execute("CREATE TABLE t (id INTEGER NOT NULL, value TEXT, active BOOLEAN NOT NULL)")
            .unwrap(),
        Outcome::Created {
            table: "t".to_owned(),
        }
    );
    database
        .execute("INSERT INTO t VALUES (1, '💾|;~%.*', TRUE)")
        .unwrap();
    database
        .execute("INSERT INTO t VALUES (2, NULL, FALSE)")
        .unwrap();

    let sql = "SELECT value, id FROM t WHERE active = TRUE AND value LIKE '💾%'";
    let plan = database.explain_select(sql).unwrap();
    assert!(!plan.pattern().is_empty());
    assert_eq!(
        database.execute(&format!("EXPLAIN REGEX {sql}")).unwrap(),
        Outcome::Explain(plan)
    );
    assert_eq!(
        rows(database.execute(sql).unwrap()),
        vec![vec![Value::Text("💾|;~%.*".to_owned()), Value::Integer(1),]]
    );

    assert_eq!(
        database
            .execute("UPDATE t SET value = 'updated' WHERE id = 2 AND value IS NULL")
            .unwrap(),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        database.execute("DELETE FROM t WHERE id = 1").unwrap(),
        Outcome::Affected { rows: 1 }
    );

    let blob = database.into_string();
    let mut reloaded = Database::from_string(blob.clone()).unwrap();
    assert_eq!(reloaded.as_str(), blob);
    assert_eq!(
        rows(reloaded.execute("SELECT * FROM t").unwrap()),
        vec![vec![
            Value::Integer(2),
            Value::Text("updated".to_owned()),
            Value::Boolean(false),
        ]]
    );
}

#[wasm_bindgen_test]
fn boolean_residuals_execute_after_reload_in_wasm() {
    let mut database = Database::new();
    database
        .execute(
            "CREATE TABLE residuals (id INTEGER NOT NULL, value TEXT, active BOOLEAN NOT NULL)",
        )
        .unwrap();
    for sql in [
        "INSERT INTO residuals VALUES (1, 'alpha', TRUE)",
        "INSERT INTO residuals VALUES (2, NULL, FALSE)",
        "INSERT INTO residuals VALUES (3, 'gamma', FALSE)",
    ] {
        database.execute(sql).unwrap();
    }
    let mut database = Database::from_string(database.into_string()).unwrap();

    assert_eq!(
        rows(
            database
                .execute("SELECT id FROM residuals WHERE (active = TRUE OR value IS NULL)",)
                .unwrap()
        ),
        vec![vec![Value::Integer(1)], vec![Value::Integer(2)]]
    );
    assert_eq!(
        database
            .execute("UPDATE residuals SET active = TRUE WHERE id = 1 OR id = 2")
            .unwrap(),
        Outcome::Affected { rows: 2 }
    );
    assert_eq!(
        database
            .execute("DELETE FROM residuals WHERE id = 2 OR id = 3")
            .unwrap(),
        Outcome::Affected { rows: 2 }
    );
    assert_eq!(
        rows(
            database
                .execute("SELECT id, active FROM residuals")
                .unwrap()
        ),
        vec![vec![Value::Integer(1), Value::Boolean(true)]]
    );
}

#[wasm_bindgen_test]
fn ordered_and_membership_predicates_execute_after_reload_in_wasm() {
    let mut database = Database::new();
    database
        .execute(
            "CREATE TABLE ordered_members (\
                 id INTEGER NOT NULL, \
                 priority INTEGER, \
                 state TEXT, \
                 enabled BOOLEAN NOT NULL\
             )",
        )
        .unwrap();
    for sql in [
        "INSERT INTO ordered_members VALUES (1, 5, 'queued', TRUE)",
        "INSERT INTO ordered_members VALUES (2, 10, 'running', TRUE)",
        "INSERT INTO ordered_members VALUES (3, 20, 'done', FALSE)",
        "INSERT INTO ordered_members VALUES (4, NULL, NULL, TRUE)",
    ] {
        database.execute(sql).unwrap();
    }
    let mut database = Database::from_string(database.into_string()).unwrap();

    assert_eq!(
        rows(
            database
                .execute(
                    "SELECT id FROM ordered_members \
                     WHERE (priority >= 10 AND state IN ('running', 'running', NULL)) \
                        OR enabled IN (FALSE)",
                )
                .unwrap()
        ),
        vec![vec![Value::Integer(2)], vec![Value::Integer(3)]]
    );
    assert!(
        rows(
            database
                .execute("SELECT id FROM ordered_members WHERE state IN (NULL, NULL)")
                .unwrap()
        )
        .is_empty()
    );
    assert_eq!(
        database
            .execute("UPDATE ordered_members SET enabled = FALSE WHERE id IN (1, 4)")
            .unwrap(),
        Outcome::Affected { rows: 2 }
    );
    assert_eq!(
        database
            .execute(
                "DELETE FROM ordered_members \
                 WHERE (priority < 10 OR enabled IN (FALSE)) AND id >= 1",
            )
            .unwrap(),
        Outcome::Affected { rows: 3 }
    );

    let mut database = Database::from_string(database.into_string()).unwrap();
    assert_eq!(
        rows(
            database
                .execute("SELECT id, state, enabled FROM ordered_members")
                .unwrap()
        ),
        vec![vec![
            Value::Integer(2),
            Value::Text("running".to_owned()),
            Value::Boolean(true),
        ]]
    );
}

#[wasm_bindgen_test]
fn malformed_storage_and_resource_limits_are_typed_in_wasm() {
    assert!(matches!(
        Database::from_string("not a database".to_owned()),
        Err(Error::CorruptStorage { .. })
    ));

    let limits = Limits {
        max_sql_bytes: 4,
        ..Limits::default()
    };
    let mut database = Database::with_limits(limits);
    let before = database.as_str().to_owned();
    assert!(matches!(
        database.execute("CREATE TABLE t (id INTEGER)"),
        Err(Error::ResourceLimit {
            resource: Resource::SqlBytes,
            limit: 4,
        })
    ));
    assert_eq!(database.as_str(), before);
}

#[wasm_bindgen_test]
fn query_output_limits_cover_rows_and_explanations_in_wasm() {
    let mut database = Database::new();
    database
        .execute("CREATE TABLE t (id INTEGER NOT NULL)")
        .unwrap();
    database.execute("INSERT INTO t VALUES (1)").unwrap();
    let blob = database.into_string();
    let row_set_limit = std::mem::size_of::<RowSet>() - 1;
    let limits = Limits {
        max_query_output_bytes: row_set_limit,
        ..Limits::default()
    };
    let mut database =
        Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");

    assert!(matches!(
        database.execute("SELECT * FROM t"),
        Err(Error::ResourceLimit {
            resource: Resource::QueryOutputBytes,
            limit,
        }) if limit == row_set_limit
    ));
    assert_eq!(database.as_str(), blob);

    let explanation_limit = std::mem::size_of::<SelectExplanation>() - 1;
    let limits = Limits {
        max_query_output_bytes: explanation_limit,
        ..Limits::default()
    };
    let mut database =
        Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");
    assert!(matches!(
        database.explain_select("SELECT * FROM t"),
        Err(Error::ResourceLimit {
            resource: Resource::QueryOutputBytes,
            limit,
        }) if limit == explanation_limit
    ));

    assert!(matches!(
        database.execute("EXPLAIN REGEX SELECT * FROM t"),
        Err(Error::ResourceLimit {
            resource: Resource::QueryOutputBytes,
            limit,
        }) if limit == explanation_limit
    ));
    assert_eq!(database.as_str(), blob);
}

#[wasm_bindgen_test]
fn like_wildcards_use_unicode_scalars_in_wasm() {
    let mut database = Database::new();
    database
        .execute("CREATE TABLE t (value TEXT NOT NULL)")
        .unwrap();
    for value in ["a💾b", "aéb", "a_b", "a%b"] {
        database
            .execute(&format!("INSERT INTO t VALUES ('{value}')"))
            .unwrap();
    }

    assert_eq!(
        rows(
            database
                .execute("SELECT value FROM t WHERE value LIKE 'a_b'")
                .unwrap()
        ),
        vec![
            vec![Value::Text("a💾b".to_owned())],
            vec![Value::Text("a_b".to_owned())],
            vec![Value::Text("a%b".to_owned())],
        ]
    );
}

#[wasm_bindgen_test]
fn ordered_collection_and_target_specific_working_boundaries_run_in_wasm() {
    let mut database = Database::new();
    database
        .execute("CREATE TABLE ordered (id INTEGER NOT NULL, key_ TEXT)")
        .unwrap();
    database
        .execute("INSERT INTO ordered VALUES (1, '💾')")
        .unwrap();
    database
        .execute("INSERT INTO ordered VALUES (2, NULL)")
        .unwrap();
    database
        .execute("INSERT INTO ordered VALUES (3, 'a')")
        .unwrap();
    let blob = database.into_string();
    let sql = "SELECT id FROM ordered ORDER BY key_";

    let mut lower = 0_usize;
    let mut upper = 8_192_usize;
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        let limits = Limits {
            max_query_working_bytes: middle,
            ..Limits::default()
        };
        let mut candidate = Database::from_string_with_limits(blob.clone(), limits).unwrap();
        if candidate.execute(sql).is_ok() {
            upper = middle;
        } else {
            lower = middle + 1;
        }
    }
    let exact = lower;
    assert!(exact > 0, "ordered collection has a nonzero working charge");

    let limits = Limits {
        max_query_working_bytes: exact,
        ..Limits::default()
    };
    let mut candidate = Database::from_string_with_limits(blob.clone(), limits).unwrap();
    assert_eq!(
        rows(candidate.execute(sql).unwrap()),
        vec![
            vec![Value::Integer(3)],
            vec![Value::Integer(1)],
            vec![Value::Integer(2)],
        ]
    );

    let limits = Limits {
        max_query_working_bytes: exact - 1,
        ..Limits::default()
    };
    let mut candidate = Database::from_string_with_limits(blob.clone(), limits).unwrap();
    assert!(matches!(
        candidate.execute(sql),
        Err(Error::ResourceLimit {
            resource: Resource::QueryWorkingBytes,
            limit,
        }) if limit == exact - 1
    ));
    assert_eq!(candidate.as_str(), blob);
}

#[wasm_bindgen_test]
fn pagination_keeps_u64_bounds_above_the_wasm_usize_range() {
    let mut database = Database::new();
    database
        .execute("CREATE TABLE pages (id INTEGER NOT NULL)")
        .unwrap();
    database.execute("INSERT INTO pages VALUES (1)").unwrap();
    database.execute("INSERT INTO pages VALUES (2)").unwrap();

    assert_eq!(
        rows(
            database
                .execute("SELECT id FROM pages LIMIT 4294967296 OFFSET 1")
                .unwrap()
        ),
        vec![vec![Value::Integer(2)]]
    );
    assert!(
        rows(
            database
                .execute("SELECT id FROM pages ORDER BY id OFFSET 4294967296")
                .unwrap()
        )
        .is_empty()
    );
    assert_eq!(
        rows(
            database
                .execute("SELECT id FROM pages LIMIT 18446744073709551615")
                .unwrap()
        ),
        vec![vec![Value::Integer(1)], vec![Value::Integer(2)]]
    );

    let blob = database.into_string();
    let limits = Limits {
        max_query_working_bytes: 0,
        ..Limits::default()
    };
    let mut database = Database::from_string_with_limits(blob, limits).unwrap();
    assert!(
        rows(
            database
                .execute(
                    "SELECT id FROM pages ORDER BY id \
                     LIMIT 0 OFFSET 18446744073709551615",
                )
                .unwrap()
        )
        .is_empty()
    );
}

#[wasm_bindgen_test]
fn primary_and_foreign_keys_survive_reload_in_wasm() {
    let mut database = Database::new();
    database
        .execute("CREATE TABLE parents (id INTEGER PRIMARY KEY)")
        .unwrap();
    database
        .execute(
            "CREATE TABLE children (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parents(id))",
        )
        .unwrap();
    database.execute("INSERT INTO parents VALUES (1)").unwrap();
    database
        .execute("INSERT INTO children VALUES (10, 1)")
        .unwrap();

    assert!(matches!(
        database.execute("INSERT INTO parents VALUES (1)"),
        Err(Error::Constraint(_))
    ));
    assert!(matches!(
        database.execute("INSERT INTO children VALUES (11, 999)"),
        Err(Error::Constraint(_))
    ));

    let blob = database.into_string();
    let mut reloaded = Database::from_string(blob.clone()).unwrap();
    assert_eq!(reloaded.as_str(), blob);
    assert!(matches!(
        reloaded.execute("DELETE FROM parents WHERE id = 1"),
        Err(Error::Constraint(_))
    ));
    assert_eq!(
        rows(reloaded.execute("SELECT * FROM children").unwrap()),
        vec![vec![Value::Integer(10), Value::Integer(1)]]
    );
}

#[wasm_bindgen_test]
fn auto_increment_high_water_survives_reload_in_wasm() {
    let mut database = Database::new();
    database
        .execute("CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT NOT NULL)")
        .unwrap();
    database
        .execute("INSERT INTO messages (body) VALUES ('first')")
        .unwrap();
    database
        .execute("INSERT INTO messages VALUES (NULL, 'second')")
        .unwrap();
    database
        .execute("DELETE FROM messages WHERE id = 2")
        .unwrap();

    let mut reloaded = Database::from_string(database.into_string()).unwrap();
    reloaded
        .execute("INSERT INTO messages (body) VALUES ('third')")
        .unwrap();
    assert_eq!(
        rows(reloaded.execute("SELECT id FROM messages").unwrap()),
        vec![vec![Value::Integer(1)], vec![Value::Integer(3)]]
    );
}

#[wasm_bindgen_test]
fn inner_joins_execute_inside_wasm() {
    let mut database = Database::new();
    database
        .execute("CREATE TABLE parents (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
        .unwrap();
    database
        .execute(
            "CREATE TABLE children (parent_id INTEGER REFERENCES parents(id), name TEXT NOT NULL)",
        )
        .unwrap();
    database
        .execute("INSERT INTO parents VALUES (1, 'parent')")
        .unwrap();
    database
        .execute("INSERT INTO children VALUES (1, 'child')")
        .unwrap();

    let sql = "SELECT parents.name, children.name FROM parents \
               JOIN children ON parents.id = children.parent_id";
    let explanation = database.explain_select(sql).unwrap();
    assert_eq!(explanation.sources(), &["parents", "children"]);
    assert_eq!(explanation.columns().len(), 2);
    assert_eq!(explanation.columns()[0].label(), "name");
    assert_eq!(explanation.columns()[0].origin().table(), "parents");
    assert_eq!(explanation.columns()[0].origin().column(), "name");
    assert_eq!(explanation.columns()[1].label(), "name");
    assert_eq!(explanation.columns()[1].origin().table(), "children");
    assert_eq!(explanation.columns()[1].origin().column(), "name");

    let row_set = match database.execute(sql).unwrap() {
        Outcome::Rows(row_set) => row_set,
        other => panic!("expected joined rows, got {other:?}"),
    };
    assert_eq!(row_set.columns(), explanation.columns());
    assert_eq!(
        row_set.into_rows(),
        vec![vec![
            Value::Text("parent".to_owned()),
            Value::Text("child".to_owned()),
        ]]
    );
}

#[wasm_bindgen_test]
fn v2_load_and_atomic_default_upgrade_work_inside_wasm() {
    let v2 = String::from("V2;~S|legacy|id:I:!;~R|legacy|I1;");
    let mut database = Database::from_string(v2.clone()).unwrap();
    assert_eq!(database.as_str(), v2);

    database
        .execute("CREATE TABLE settings (enabled BOOLEAN DEFAULT TRUE, note TEXT DEFAULT NULL)")
        .unwrap();
    assert!(database.as_str().starts_with("V3;"));
    assert!(database.as_str().contains("~D|settings|enabled|B1;"));
    assert!(database.as_str().contains("~D|settings|note|N;"));
    database
        .execute("INSERT INTO settings (note) VALUES (NULL)")
        .unwrap();

    let blob = database.into_string();
    let mut reloaded = Database::from_string(blob.clone()).unwrap();
    assert_eq!(reloaded.as_str(), blob);
    assert_eq!(
        rows(
            reloaded
                .execute("SELECT enabled, note FROM settings")
                .unwrap()
        ),
        vec![vec![Value::Boolean(true), Value::Null]]
    );
}

#[wasm_bindgen_test]
fn unique_constraints_persist_and_validate_inside_wasm() {
    let mut database = Database::new();
    database
        .execute("CREATE TABLE accounts (email TEXT UNIQUE)")
        .unwrap();
    database
        .execute("INSERT INTO accounts VALUES ('one@example.com')")
        .unwrap();
    database
        .execute("INSERT INTO accounts VALUES (NULL)")
        .unwrap();
    database
        .execute("INSERT INTO accounts VALUES (NULL)")
        .unwrap();
    assert!(matches!(
        database.execute("INSERT INTO accounts VALUES ('one@example.com')"),
        Err(varchar::Error::Constraint(_))
    ));

    let blob = database.into_string();
    let reloaded = Database::from_string(blob.clone()).unwrap();
    assert_eq!(reloaded.as_str(), blob);
    assert!(reloaded.as_str().contains("~U|accounts|email;"));
}

#[wasm_bindgen_test]
fn all_null_unique_columns_fit_the_exact_wasm_database_limit() {
    const COLUMN_COUNT: usize = 100;
    const ROW_COUNT: usize = 100;

    let mut blob = String::from("V3;~S|t");
    for column in 0..COLUMN_COUNT {
        blob.push_str(&format!("|c{column}:I:?"));
    }
    blob.push(';');
    for column in 0..COLUMN_COUNT {
        blob.push_str(&format!("~U|t|c{column};"));
    }
    for _ in 0..ROW_COUNT {
        blob.push_str("~R|t");
        for _ in 0..COLUMN_COUNT {
            blob.push_str("|N");
        }
        blob.push(';');
    }

    let limits = Limits {
        max_database_bytes: blob.len(),
        ..Limits::default()
    };
    let database = Database::from_string_with_limits(blob.clone(), limits).unwrap();
    assert_eq!(database.as_str(), blob);
}

#[wasm_bindgen_test]
fn compact_measurements_allow_shrinking_multi_row_update_in_wasm() {
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
        database.execute("UPDATE t SET body = ''").unwrap(),
        Outcome::Affected { rows: 8 }
    );
    assert_eq!(
        rows(database.execute("SELECT body FROM t ORDER BY id").unwrap()),
        vec![vec![Value::Text(String::new())]; 8]
    );
}

#[wasm_bindgen_test]
fn check_like_work_limits_mutation_and_reload_inside_wasm() {
    // An interior literal run is retried at every candidate start, so it is the
    // shape that charges the backtracking budget. Anchored prefixes and
    // suffixes are matched in one pass and deliberately cost nothing.
    const CHECKED: &str =
        "CREATE TABLE patterns (value TEXT CHECK (value LIKE '%aaaaaaaaaab%' OR value = 'exempt'))";
    let limits = Limits {
        regex_backtrack_limit: 10,
        ..Limits::default()
    };
    let mut value = "a".repeat(4_096);
    value.push('b');
    let insert = format!("INSERT INTO patterns VALUES ('{value}')");

    let mut database = Database::with_limits(limits.clone());
    database.execute(CHECKED).unwrap();
    let before = database.as_str().to_owned();
    assert!(matches!(
        database.execute(&insert),
        Err(Error::ResourceLimit {
            resource: Resource::RegexBacktracking,
            limit: 10,
        })
    ));
    assert_eq!(database.as_str(), before);

    let mut permissive = Database::new();
    permissive.execute(CHECKED).unwrap();
    permissive.execute(&insert).unwrap();
    assert!(matches!(
        Database::from_string_with_limits(permissive.into_string(), limits),
        Err(Error::ResourceLimit {
            resource: Resource::RegexBacktracking,
            limit: 10,
        })
    ));
}

#[wasm_bindgen_test]
fn check_constraints_persist_validate_and_rollback_inside_wasm() {
    let mut database = Database::new();
    database
        .execute(
            "CREATE TABLE tasks (id INTEGER PRIMARY KEY AUTOINCREMENT CHECK (id > 0), \
             state TEXT DEFAULT 'queued', \
             attempts INTEGER CHECK (attempts >= 0 OR attempts IS NULL), \
             CHECK (state IN ('queued', 'running') AND state LIKE '%'))",
        )
        .unwrap();
    database
        .execute("INSERT INTO tasks (attempts) VALUES (NULL)")
        .unwrap();
    database
        .execute("INSERT INTO tasks (state, attempts) VALUES ('running', 2)")
        .unwrap();

    let before_failed_insert = database.as_str().to_owned();
    assert!(matches!(
        database.execute("INSERT INTO tasks (attempts) VALUES (-1)"),
        Err(Error::Constraint(_))
    ));
    assert_eq!(database.as_str(), before_failed_insert);

    let blob = database.into_string();
    let mut reloaded = Database::from_string(blob.clone()).unwrap();
    assert_eq!(reloaded.as_str(), blob);
    assert!(reloaded.as_str().contains("~C|tasks|"));

    assert!(matches!(
        reloaded.execute("UPDATE tasks SET state = 'stopped' WHERE id = 2"),
        Err(Error::Constraint(_))
    ));
    assert_eq!(reloaded.as_str(), blob);
    assert_eq!(
        rows(
            reloaded
                .execute("SELECT id, state, attempts FROM tasks")
                .unwrap()
        ),
        vec![
            vec![
                Value::Integer(1),
                Value::Text("queued".to_owned()),
                Value::Null
            ],
            vec![
                Value::Integer(2),
                Value::Text("running".to_owned()),
                Value::Integer(2),
            ],
        ]
    );

    let metadata_end = blob.find("~R|").expect("the fixture contains rows");
    let metadata = &blob[..metadata_end];
    assert_eq!(
        reloaded
            .execute("UPDATE tasks SET state = 'running', attempts = 5")
            .unwrap(),
        Outcome::Affected { rows: 2 }
    );
    assert!(reloaded.as_str().starts_with(metadata));

    let before_zero_match = reloaded.as_str().to_owned();
    assert_eq!(
        reloaded
            .execute("UPDATE tasks SET id = 50 WHERE id = 999")
            .unwrap(),
        Outcome::Affected { rows: 0 }
    );
    assert_eq!(reloaded.as_str(), before_zero_match);

    reloaded
        .execute("INSERT INTO tasks (attempts) VALUES (0)")
        .unwrap();
    assert_eq!(
        rows(
            reloaded
                .execute("SELECT id FROM tasks ORDER BY id")
                .unwrap()
        ),
        vec![
            vec![Value::Integer(1)],
            vec![Value::Integer(2)],
            vec![Value::Integer(3)],
        ]
    );
}

#[wasm_bindgen_test]
fn escaped_check_create_honors_exact_and_one_under_database_limits_in_wasm() {
    let sql =
        "CREATE TABLE escape_bound (value TEXT CHECK (value LIKE '\\%\\_\\\\|;~\u{2028}\u{2029}'))";
    let mut probe = Database::new();
    probe.execute(sql).unwrap();
    let expected = probe.into_string();

    let mut exact = Database::with_limits(Limits {
        max_database_bytes: expected.len(),
        ..Limits::default()
    });
    exact.execute(sql).unwrap();
    assert_eq!(exact.as_str(), expected);

    let lower_limit = expected.len() - 1;
    let mut lower = Database::with_limits(Limits {
        max_database_bytes: lower_limit,
        ..Limits::default()
    });
    assert!(matches!(
        lower.execute(sql),
        Err(Error::ResourceLimit {
            resource: Resource::DatabaseBytes,
            limit,
        }) if limit == lower_limit
    ));
    assert_eq!(lower.as_str(), "V2;");
    lower.execute("CREATE TABLE ok (id INTEGER)").unwrap();
    assert_eq!(lower.as_str(), "V2;~S|ok|id:I:?;");
}

#[wasm_bindgen_test]
fn update_cascade_has_exact_wasm_working_boundaries_and_atomic_rollback() {
    const CHILDREN: usize = 32;
    let mut source = String::from(
        "V3;~S|p|id:I:!;~P|p|id;\
         ~S|c|parent_id:I:!;~F|c|parent_id|p|id|R|C;\
         ~R|p|I0;",
    );
    for _ in 0..CHILDREN {
        source.push_str("~R|c|I0;");
    }
    let sql = "UPDATE p SET id = 1 WHERE id = 0";

    let mut lower = source.len();
    let mut upper = source.len().saturating_mul(16);
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        let limits = Limits {
            max_database_bytes: middle,
            ..Limits::default()
        };
        let mut candidate = Database::from_string_with_limits(source.clone(), limits).unwrap();
        if candidate.execute(sql).is_ok() {
            upper = middle;
        } else {
            lower = middle + 1;
        }
    }
    let exact = lower;
    assert!(
        exact > source.len(),
        "cascade working state exceeds the exact source-derived budget"
    );

    let mut exact_database = Database::from_string_with_limits(
        source.clone(),
        Limits {
            max_database_bytes: exact,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(
        exact_database.execute(sql).unwrap(),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(exact_database.execute("SELECT parent_id FROM c").unwrap()),
        vec![vec![Value::Integer(1)]; CHILDREN]
    );

    let one_under = exact - 1;
    let mut limited = Database::from_string_with_limits(
        source.clone(),
        Limits {
            max_database_bytes: one_under,
            ..Limits::default()
        },
    )
    .unwrap();
    assert!(matches!(
        limited.execute(sql),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            ..
        })
    ));
    assert_eq!(limited.as_str(), source);
}

#[wasm_bindgen_test]
fn referential_actions_execute_rollback_and_reload_inside_wasm() {
    let mut database = Database::new();
    database
        .execute("CREATE TABLE parents (id INTEGER PRIMARY KEY)")
        .unwrap();
    database
        .execute(
            "CREATE TABLE cascade_children (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parents(id) ON DELETE CASCADE)",
        )
        .unwrap();
    database
        .execute(
            "CREATE TABLE nullable_children (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parents(id) ON DELETE SET NULL ON UPDATE RESTRICT)",
        )
        .unwrap();
    database
        .execute(
            "CREATE TABLE restricted_children (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parents(id) ON DELETE RESTRICT ON UPDATE RESTRICT)",
        )
        .unwrap();
    database
        .execute(
            "CREATE TABLE update_children (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parents(id) ON UPDATE CASCADE)",
        )
        .unwrap();
    for sql in [
        "INSERT INTO parents VALUES (1)",
        "INSERT INTO parents VALUES (2)",
        "INSERT INTO parents VALUES (3)",
        "INSERT INTO parents VALUES (4)",
        "INSERT INTO cascade_children VALUES (10, 1)",
        "INSERT INTO nullable_children VALUES (20, 2)",
        "INSERT INTO restricted_children VALUES (30, 3)",
        "INSERT INTO update_children VALUES (40, 4)",
    ] {
        database.execute(sql).unwrap();
    }

    let blob = database.into_string();
    let mut database = Database::from_string(blob.clone()).unwrap();
    assert_eq!(database.as_str(), blob);
    assert_eq!(
        database
            .execute("DELETE FROM parents WHERE id = 1")
            .unwrap(),
        Outcome::Affected { rows: 1 }
    );
    assert!(rows(database.execute("SELECT * FROM cascade_children").unwrap()).is_empty());
    assert_eq!(
        database
            .execute("DELETE FROM parents WHERE id = 2")
            .unwrap(),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(
            database
                .execute("SELECT parent_id FROM nullable_children")
                .unwrap()
        ),
        vec![vec![Value::Null]]
    );

    let before_restrict = database.as_str().to_owned();
    assert!(matches!(
        database.execute("DELETE FROM parents WHERE id = 3"),
        Err(Error::Constraint(_))
    ));
    assert_eq!(database.as_str(), before_restrict);
    assert_eq!(
        database
            .execute("UPDATE parents SET id = 44 WHERE id = 4")
            .unwrap(),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(
            database
                .execute("SELECT parent_id FROM update_children")
                .unwrap()
        ),
        vec![vec![Value::Integer(44)]]
    );

    let mut reloaded = Database::from_string(database.into_string()).unwrap();
    assert_eq!(
        rows(
            reloaded
                .execute("SELECT id FROM parents ORDER BY id")
                .unwrap()
        ),
        vec![vec![Value::Integer(3)], vec![Value::Integer(44)]]
    );
    assert_eq!(
        rows(
            reloaded
                .execute("SELECT parent_id FROM update_children")
                .unwrap()
        ),
        vec![vec![Value::Integer(44)]]
    );
}

#[wasm_bindgen_test]
fn schema_metadata_results_are_bounded_and_reload_inside_wasm() {
    let mut database = Database::new();
    database
        .execute(
            "CREATE TABLE accounts (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT DEFAULT 'seed')",
        )
        .unwrap();
    let source = database.into_string();
    let mut reloaded = Database::from_string(source.clone()).unwrap();

    assert_eq!(
        rows(reloaded.execute("SHOW TABLES").unwrap()),
        vec![vec![Value::Text(String::from("accounts"))]]
    );
    let description = rows(reloaded.execute("DESCRIBE accounts").unwrap());
    assert_eq!(description.len(), 2);
    assert_eq!(description[0][0], Value::Text(String::from("id")));
    assert_eq!(description[0][3], Value::Boolean(true));
    assert_eq!(description[0][6], Value::Boolean(true));
    assert_eq!(description[1][5], Value::Text(String::from("'seed'")));
    assert_eq!(
        rows(reloaded.execute("SHOW CREATE TABLE accounts").unwrap()),
        vec![vec![
            Value::Text(String::from("accounts")),
            Value::Text(String::from(
                "CREATE TABLE accounts (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, \
                 name TEXT DEFAULT 'seed')",
            )),
        ]]
    );
    assert_eq!(reloaded.as_str(), source);

    // `default_value` must round-trip as the SQL literal that reproduces the
    // default, so a TEXT default holding `NULL` stays distinguishable from an
    // explicit `DEFAULT NULL` and from a column with no default at all.
    reloaded
        .execute(
            "CREATE TABLE defaults (\
                quoted_null TEXT DEFAULT 'NULL', \
                absent TEXT, \
                literal_null TEXT DEFAULT NULL, \
                literal_true BOOLEAN DEFAULT TRUE, \
                literal_digits INTEGER DEFAULT 5, \
                apostrophes TEXT DEFAULT 'it''s'\
            )",
        )
        .unwrap();
    assert_eq!(
        rows(reloaded.execute("DESCRIBE defaults").unwrap())
            .into_iter()
            .map(|row| row[5].clone())
            .collect::<Vec<_>>(),
        vec![
            Value::Text(String::from("'NULL'")),
            Value::Null,
            Value::Text(String::from("NULL")),
            Value::Text(String::from("TRUE")),
            Value::Text(String::from("5")),
            Value::Text(String::from("'it''s'")),
        ]
    );

    let limit = std::mem::size_of::<RowSet>() - 1;
    for sql in [
        "SHOW TABLES",
        "DESCRIBE accounts",
        "SHOW CREATE TABLE accounts",
    ] {
        let mut limited = Database::from_string_with_limits(
            source.clone(),
            Limits {
                max_query_output_bytes: limit,
                ..Limits::default()
            },
        )
        .unwrap();
        assert!(matches!(
            limited.execute(sql),
            Err(Error::ResourceLimit {
                resource: Resource::QueryOutputBytes,
                limit: actual,
            }) if actual == limit
        ));
        assert_eq!(limited.as_str(), source);
    }

    let sql = "SHOW CREATE TABLE accounts";
    let exact = minimum_output_limit(&source, sql);
    let mut exact_database = Database::from_string_with_limits(
        source.clone(),
        Limits {
            max_query_output_bytes: exact,
            max_query_working_bytes: 0,
            ..Limits::default()
        },
    )
    .unwrap();
    assert!(matches!(exact_database.execute(sql), Ok(Outcome::Rows(_))));

    let one_under = exact - 1;
    let mut limited = Database::from_string_with_limits(
        source.clone(),
        Limits {
            max_query_output_bytes: one_under,
            max_query_working_bytes: 0,
            ..Limits::default()
        },
    )
    .unwrap();
    assert!(matches!(
        limited.execute(sql),
        Err(Error::ResourceLimit {
            resource: Resource::QueryOutputBytes,
            limit: actual,
        }) if actual == one_under
    ));
    assert_eq!(limited.as_str(), source);
}

fn minimum_output_limit(source: &str, sql: &str) -> usize {
    let mut upper = 1_usize;
    loop {
        let mut database = Database::from_string_with_limits(
            source.to_owned(),
            Limits {
                max_query_output_bytes: upper,
                ..Limits::default()
            },
        )
        .unwrap();
        if database.execute(sql).is_ok() {
            break;
        }
        upper = upper.checked_mul(2).expect("metadata output fits usize");
    }

    let mut lower = 0;
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        let mut database = Database::from_string_with_limits(
            source.to_owned(),
            Limits {
                max_query_output_bytes: middle,
                ..Limits::default()
            },
        )
        .unwrap();
        if database.execute(sql).is_ok() {
            upper = middle;
        } else {
            lower = middle + 1;
        }
    }
    lower
}
