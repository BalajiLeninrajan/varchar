use varchar::{Database, Error, Limits, Outcome, Resource, Value};

fn execute(database: &mut Database, sql: &str) -> Outcome {
    database
        .execute(sql)
        .unwrap_or_else(|error| panic!("failed to execute {sql:?}: {error}"))
}

fn rows(database: &mut Database, sql: &str) -> Vec<Vec<Value>> {
    match execute(database, sql) {
        Outcome::Rows(rows) => rows.into_rows(),
        other => panic!("expected rows for {sql:?}, got {other:?}"),
    }
}

fn ids(database: &mut Database, sql: &str) -> Vec<i64> {
    rows(database, sql)
        .into_iter()
        .map(|row| match row.as_slice() {
            [Value::Integer(id)] => *id,
            other => panic!("expected one INTEGER, got {other:?}"),
        })
        .collect()
}

fn scalar_fixture() -> Database {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE records (id INTEGER NOT NULL, n INTEGER, flag BOOLEAN, name TEXT)",
    );
    for sql in [
        "INSERT INTO records VALUES (1, 2, TRUE, 'é')",
        "INSERT INTO records VALUES (2, NULL, FALSE, '💾')",
        "INSERT INTO records VALUES (3, -1, NULL, 'é')",
        "INSERT INTO records VALUES (4, 2, FALSE, NULL)",
        "INSERT INTO records VALUES (5, 2, FALSE, 'a')",
    ] {
        execute(&mut database, sql);
    }
    database
}

#[test]
fn orders_every_scalar_type_with_fixed_null_placement() {
    let mut database = scalar_fixture();

    assert_eq!(
        ids(&mut database, "SELECT id FROM records ORDER BY n"),
        [3, 1, 4, 5, 2]
    );
    assert_eq!(
        ids(&mut database, "SELECT id FROM records ORDER BY n DESC"),
        [2, 1, 4, 5, 3]
    );
    assert_eq!(
        ids(&mut database, "SELECT id FROM records ORDER BY flag ASC"),
        [2, 4, 5, 1, 3]
    );
    assert_eq!(
        ids(&mut database, "SELECT id FROM records ORDER BY flag DESC"),
        [3, 1, 2, 4, 5]
    );
    assert_eq!(
        ids(&mut database, "SELECT id FROM records ORDER BY name"),
        [5, 3, 1, 2, 4]
    );
    assert_eq!(
        ids(&mut database, "SELECT id FROM records ORDER BY name DESC"),
        [4, 2, 1, 3, 5]
    );
}

#[test]
fn supports_multi_key_hidden_duplicate_terms_and_duplicate_projection() {
    let mut database = scalar_fixture();
    let before = database.as_str().to_owned();

    assert_eq!(
        ids(
            &mut database,
            "SELECT id FROM records ORDER BY n ASC, id DESC, n ASC",
        ),
        [3, 5, 4, 1, 2]
    );
    assert_eq!(
        rows(
            &mut database,
            "SELECT name, id, name FROM records ORDER BY n, name DESC",
        ),
        vec![
            vec![
                Value::Text("é".to_owned()),
                Value::Integer(3),
                Value::Text("é".to_owned()),
            ],
            vec![Value::Null, Value::Integer(4), Value::Null],
            vec![
                Value::Text("é".to_owned()),
                Value::Integer(1),
                Value::Text("é".to_owned()),
            ],
            vec![
                Value::Text("a".to_owned()),
                Value::Integer(5),
                Value::Text("a".to_owned()),
            ],
            vec![
                Value::Text("💾".to_owned()),
                Value::Integer(2),
                Value::Text("💾".to_owned()),
            ],
        ]
    );
    assert_eq!(
        database.as_str(),
        before,
        "ordered reads never mutate storage"
    );

    let blob = database.into_string();
    let mut reloaded = Database::from_string(blob.clone()).expect("fixture reloads");
    assert_eq!(
        ids(&mut reloaded, "SELECT id FROM records ORDER BY n, id DESC"),
        [3, 5, 4, 1, 2]
    );
    assert_eq!(reloaded.as_str(), blob);
}

#[test]
fn joined_ordering_uses_real_qualified_sources_and_preserves_final_ties() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER NOT NULL, rank INTEGER NOT NULL)",
    );
    execute(
        &mut database,
        "CREATE TABLE children (id INTEGER NOT NULL, parent_id INTEGER NOT NULL, name TEXT NOT NULL)",
    );
    for sql in [
        "INSERT INTO parents VALUES (2, 1)",
        "INSERT INTO parents VALUES (1, 1)",
        "INSERT INTO children VALUES (10, 1, 'alpha')",
        "INSERT INTO children VALUES (11, 1, 'beta')",
        "INSERT INTO children VALUES (12, 2, 'gamma')",
    ] {
        execute(&mut database, sql);
    }

    let join = "FROM parents JOIN children ON parents.id = children.parent_id";
    assert_eq!(
        rows(
            &mut database,
            &format!("SELECT parents.id, children.id {join} ORDER BY parents.rank"),
        ),
        vec![
            vec![Value::Integer(2), Value::Integer(12)],
            vec![Value::Integer(1), Value::Integer(10)],
            vec![Value::Integer(1), Value::Integer(11)],
        ]
    );
    assert_eq!(
        rows(
            &mut database,
            &format!(
                "SELECT children.name {join} \
                 ORDER BY parents.id, children.name DESC"
            ),
        ),
        vec![
            vec![Value::Text("beta".to_owned())],
            vec![Value::Text("alpha".to_owned())],
            vec![Value::Text("gamma".to_owned())],
        ]
    );

    assert!(matches!(
        database.execute(&format!("SELECT parents.id {join} ORDER BY id")),
        Err(Error::Schema(message))
            if message == "ambiguous column \"id\"; qualify it with a table name"
    ));
    assert!(matches!(
        database.execute(&format!("SELECT parents.id {join} ORDER BY alias.id")),
        Err(Error::Schema(message)) if message == "unknown table qualifier \"alias\""
    ));
}

#[test]
fn unordered_queries_keep_streaming_order_and_avoid_ordered_retention() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE stream (id INTEGER NOT NULL, note TEXT NOT NULL)",
    );
    for id in 0..8 {
        execute(
            &mut database,
            &format!("INSERT INTO stream VALUES ({id}, 'x')"),
        );
    }
    let blob = database.into_string();
    let limits = Limits {
        max_query_working_bytes: 512,
        ..Limits::default()
    };
    let mut limited =
        Database::from_string_with_limits(blob.clone(), limits).expect("fixture reloads");

    assert_eq!(
        ids(&mut limited, "SELECT id FROM stream"),
        [0, 1, 2, 3, 4, 5, 6, 7]
    );
    assert!(matches!(
        limited.execute("SELECT id FROM stream ORDER BY id"),
        Err(Error::ResourceLimit {
            resource: Resource::QueryWorkingBytes,
            limit: 512,
        })
    ));
    assert_eq!(limited.as_str(), blob);
}

#[test]
fn explain_resolves_order_terms_without_changing_the_scan_prefilter() {
    let database = scalar_fixture();
    let plain = database
        .explain_select("SELECT id FROM records WHERE flag = TRUE")
        .expect("plain query explains");
    let ordered = database
        .explain_select("SELECT id FROM records WHERE flag = TRUE ORDER BY name DESC")
        .expect("ordered query explains");

    assert_eq!(ordered.pattern(), plain.pattern());
    assert_eq!(ordered.sources(), plain.sources());
    assert_eq!(ordered.columns(), plain.columns());
}
