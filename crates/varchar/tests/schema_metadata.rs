use varchar::{DataType, Database, Error, Limits, Outcome, Resource, RowSet, Value};

fn execute(database: &mut Database, sql: &str) -> Outcome {
    database
        .execute(sql)
        .unwrap_or_else(|error| panic!("failed to execute {sql:?}: {error}"))
}

fn row_set(database: &mut Database, sql: &str) -> RowSet {
    match execute(database, sql) {
        Outcome::Rows(rows) => rows,
        other => panic!("expected rows, got {other:?}"),
    }
}

#[test]
fn show_tables_returns_catalog_order_with_virtual_column_metadata() {
    let mut database = Database::new();
    let before = database.as_str().to_owned();
    let empty = row_set(&mut database, "SHOW TABLES");
    assert!(empty.rows().is_empty());
    assert_eq!(database.as_str(), before);
    assert_eq!(empty.columns().len(), 1);
    assert_eq!(empty.columns()[0].label(), "table_name");
    assert_eq!(
        empty.columns()[0].origin().table(),
        "information_schema.tables"
    );
    assert_eq!(empty.columns()[0].origin().column(), "table_name");
    assert_eq!(empty.columns()[0].data_type(), DataType::Text);
    assert!(!empty.columns()[0].nullable());

    execute(&mut database, "CREATE TABLE zebra (id INTEGER)");
    execute(&mut database, "CREATE TABLE alpha (id INTEGER)");
    execute(&mut database, "CREATE TABLE middle (id INTEGER)");
    let before = database.as_str().to_owned();
    assert_eq!(
        row_set(&mut database, "sHoW tAbLeS;").into_rows(),
        vec![
            vec![Value::Text(String::from("zebra"))],
            vec![Value::Text(String::from("alpha"))],
            vec![Value::Text(String::from("middle"))],
        ]
    );
    assert_eq!(database.as_str(), before);

    let mut reloaded = Database::from_string(database.into_string()).expect("metadata reloads");
    assert_eq!(
        row_set(&mut reloaded, "SHOW TABLES").into_rows(),
        vec![
            vec![Value::Text(String::from("zebra"))],
            vec![Value::Text(String::from("alpha"))],
            vec![Value::Text(String::from("middle"))],
        ]
    );
}

#[test]
fn metadata_statement_words_are_reserved_identifiers() {
    let mut database = Database::new();
    for (sql, keyword) in [
        ("CREATE TABLE show (id INTEGER)", "SHOW"),
        ("CREATE TABLE t (tables TEXT)", "TABLES"),
    ] {
        assert!(
            matches!(
                database.execute(sql),
                Err(Error::Parse { ref message, .. })
                    if message == &format!(
                        "reserved keyword `{keyword}` cannot be used as an identifier"
                    )
            ),
            "expected {keyword} to be reserved for {sql:?}"
        );
    }
}

#[test]
fn metadata_results_obey_exact_output_boundaries_without_query_working_state() {
    let mut database = Database::new();
    execute(&mut database, "CREATE TABLE second (value TEXT)");
    execute(
        &mut database,
        "CREATE TABLE first (id INTEGER PRIMARY KEY AUTO_INCREMENT, name TEXT DEFAULT 'seed')",
    );
    let source = database.into_string();

    let sql = "SHOW TABLES";
    let exact = minimum_output_limit(&source, sql);
    let mut exact_database = Database::from_string_with_limits(
        source.clone(),
        Limits {
            max_query_output_bytes: exact,
            max_query_working_bytes: 0,
            ..Limits::default()
        },
    )
    .expect("the metadata fixture loads");
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
    .expect("the metadata fixture loads below the output boundary");
    assert!(matches!(
        limited.execute(sql),
        Err(Error::ResourceLimit {
            resource: Resource::QueryOutputBytes,
            limit,
        }) if limit == one_under
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
        .expect("the metadata fixture loads while searching the output bound");
        if database.execute(sql).is_ok() {
            break;
        }
        upper = upper
            .checked_mul(2)
            .expect("the metadata output boundary fits usize");
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
        .expect("the metadata fixture loads while searching the output bound");
        if database.execute(sql).is_ok() {
            upper = middle;
        } else {
            lower = middle + 1;
        }
    }
    lower
}
