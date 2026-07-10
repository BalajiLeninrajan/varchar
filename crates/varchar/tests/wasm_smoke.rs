#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

use varchar::{Database, Error, Limits, Outcome, Value};
use wasm_bindgen_test::wasm_bindgen_test;

fn rows(outcome: Outcome) -> Vec<Vec<Value>> {
    match outcome {
        Outcome::Rows(row_set) => row_set.rows,
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
    let plan = database.compile_select(sql).unwrap();
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
        Err(Error::ResourceLimit { limit: 4, .. })
    ));
    assert_eq!(database.as_str(), before);
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
