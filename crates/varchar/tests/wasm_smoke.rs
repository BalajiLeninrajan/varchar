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
