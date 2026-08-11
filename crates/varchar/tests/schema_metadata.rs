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
        ("CREATE TABLE t (describe INTEGER)", "DESCRIBE"),
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

    // Quoting stays the escape hatch, and SHOW CREATE TABLE replays it verbatim.
    execute(
        &mut database,
        "CREATE TABLE \"show\" (\"describe\" INTEGER, \"tables\" TEXT)",
    );
    execute(&mut database, "INSERT INTO \"show\" VALUES (7, 'kept')");

    let mut reloaded =
        Database::from_string(database.into_string()).expect("quoted metadata identifiers reload");
    assert_eq!(
        row_set(
            &mut reloaded,
            "SELECT \"describe\", \"tables\" FROM \"show\""
        )
        .into_rows(),
        vec![vec![Value::Integer(7), Value::Text(String::from("kept"))]]
    );
    assert_eq!(
        row_set(&mut reloaded, "DESCRIBE \"show\"").rows()[0][0],
        Value::Text(String::from("describe"))
    );
    assert_eq!(
        row_set(&mut reloaded, "SHOW CREATE TABLE \"show\"").rows()[0][1],
        Value::Text(String::from(
            "CREATE TABLE \"show\" (\"describe\" INTEGER, \"tables\" TEXT)",
        ))
    );
}

#[test]
fn show_create_quotes_reserved_catalog_identifiers_for_replay() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE \"select\" (\"order\" INTEGER PRIMARY KEY)",
    );
    execute(
        &mut database,
        "CREATE TABLE \"from\" (\
            \"where\" INTEGER REFERENCES \"select\"(\"order\"), \
            \"group\" TEXT, \
            CHECK (\"group\" != '')\
        )",
    );

    let parent = row_set(&mut database, "SHOW CREATE TABLE \"select\"").into_rows()[0][1].clone();
    let child = row_set(&mut database, "SHOW CREATE TABLE \"from\"").into_rows()[0][1].clone();
    assert_eq!(
        parent,
        Value::Text(String::from(
            "CREATE TABLE \"select\" (\"order\" INTEGER NOT NULL PRIMARY KEY)"
        ))
    );
    assert_eq!(
        child,
        Value::Text(String::from(
            "CREATE TABLE \"from\" (\"where\" INTEGER REFERENCES \"select\"(\"order\") \
             ON DELETE RESTRICT ON UPDATE RESTRICT, \"group\" TEXT, CHECK (\"group\" != ''))"
        ))
    );

    let mut recreated = Database::new();
    let Value::Text(parent) = parent else {
        unreachable!("SHOW CREATE returns text")
    };
    let Value::Text(child) = child else {
        unreachable!("SHOW CREATE returns text")
    };
    execute(&mut recreated, &parent);
    execute(&mut recreated, &child);
    assert_eq!(
        row_set(&mut recreated, "SHOW CREATE TABLE \"from\"").rows()[0][1],
        Value::Text(child)
    );
}

#[test]
fn show_create_replays_quoted_clause_words_beside_the_clauses_they_name() {
    let mut database = Database::new();

    // One predicate drives both consumers, so the words the parser refuses as
    // bare identifiers are exactly the words the writer has to quote back.
    assert!(
        matches!(
            database.execute("CREATE TABLE t (default TEXT DEFAULT 'pending')"),
            Err(Error::Parse { ref message, .. })
                if message == "reserved keyword `DEFAULT` cannot be used as an identifier"
        ),
        "expected DEFAULT to be reserved as a column name"
    );

    execute(
        &mut database,
        "CREATE TABLE \"check\" (\
            \"default\" TEXT DEFAULT 'pending', \
            \"unique\" INTEGER UNIQUE DEFAULT 0, \
            \"offset\" INTEGER DEFAULT 5, \
            CHECK (\"default\" != '')\
        )",
    );

    let ddl = row_set(&mut database, "SHOW CREATE TABLE \"check\"").into_rows()[0][1].clone();
    assert_eq!(
        ddl,
        Value::Text(String::from(
            "CREATE TABLE \"check\" (\"default\" TEXT DEFAULT 'pending', \
             \"unique\" INTEGER UNIQUE DEFAULT 0, \"offset\" INTEGER DEFAULT 5, \
             CHECK (\"default\" != ''))"
        ))
    );

    let Value::Text(ddl) = ddl else {
        unreachable!("SHOW CREATE returns text")
    };
    let mut recreated = Database::new();
    execute(&mut recreated, &ddl);
    execute(
        &mut recreated,
        "INSERT INTO \"check\" (\"offset\") VALUES (9)",
    );
    assert_eq!(
        row_set(
            &mut recreated,
            "SELECT \"default\", \"unique\", \"offset\" FROM \"check\""
        )
        .into_rows(),
        vec![vec![
            Value::Text(String::from("pending")),
            Value::Integer(0),
            Value::Integer(9),
        ]]
    );
    assert_eq!(
        row_set(&mut recreated, "SHOW CREATE TABLE \"check\"").rows()[0][1],
        Value::Text(ddl)
    );
}

#[test]
fn describe_reports_column_order_types_defaults_and_single_column_keys() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY)",
    );
    execute(
        &mut database,
        "CREATE TABLE widgets (\
            id INTEGER PRIMARY KEY AUTO_INCREMENT, \
            email TEXT NOT NULL UNIQUE DEFAULT 'seed', \
            active BOOLEAN DEFAULT TRUE, \
            note TEXT DEFAULT NULL, \
            minimum INTEGER DEFAULT -9223372036854775808, \
            parent_id INTEGER REFERENCES parents(id)\
        )",
    );
    let before = database.as_str().to_owned();
    let description = row_set(&mut database, "DESCRIBE Widgets");
    assert_eq!(database.as_str(), before);

    let labels = description
        .columns()
        .iter()
        .map(|column| column.label())
        .collect::<Vec<_>>();
    assert_eq!(
        labels,
        vec![
            "column_name",
            "data_type",
            "nullable",
            "primary_key",
            "unique",
            "default_value",
            "auto_increment",
        ]
    );
    assert_eq!(
        description
            .columns()
            .iter()
            .map(|column| column.data_type())
            .collect::<Vec<_>>(),
        vec![
            DataType::Text,
            DataType::Text,
            DataType::Boolean,
            DataType::Boolean,
            DataType::Boolean,
            DataType::Text,
            DataType::Boolean,
        ]
    );
    assert_eq!(
        description
            .columns()
            .iter()
            .map(|column| column.nullable())
            .collect::<Vec<_>>(),
        vec![false, false, false, false, false, true, false]
    );
    assert!(
        description
            .columns()
            .iter()
            .all(|column| column.origin().table() == "information_schema.columns")
    );
    assert_eq!(
        description.into_rows(),
        vec![
            describe_row("id", "INTEGER", false, true, true, Value::Null, true),
            describe_row(
                "email",
                "TEXT",
                false,
                false,
                true,
                Value::Text(String::from("'seed'")),
                false,
            ),
            describe_row(
                "active",
                "BOOLEAN",
                true,
                false,
                false,
                Value::Text(String::from("TRUE")),
                false,
            ),
            describe_row(
                "note",
                "TEXT",
                true,
                false,
                false,
                Value::Text(String::from("NULL")),
                false,
            ),
            describe_row(
                "minimum",
                "INTEGER",
                true,
                false,
                false,
                Value::Text(String::from("-9223372036854775808")),
                false,
            ),
            describe_row(
                "parent_id",
                "INTEGER",
                true,
                false,
                false,
                Value::Null,
                false,
            ),
        ]
    );
}

#[test]
fn describe_renders_defaults_as_sql_literals_rather_than_display_text() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE t (\
            quoted_null TEXT DEFAULT 'NULL', \
            absent TEXT, \
            literal_null TEXT DEFAULT NULL, \
            quoted_true TEXT DEFAULT 'TRUE', \
            literal_true BOOLEAN DEFAULT TRUE, \
            quoted_digits TEXT DEFAULT '5', \
            literal_digits INTEGER DEFAULT 5, \
            apostrophes TEXT DEFAULT 'it''s'\
        )",
    );

    let defaults = row_set(&mut database, "DESCRIBE t")
        .into_rows()
        .into_iter()
        .map(|row| row[5].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        defaults,
        vec![
            Value::Text(String::from("'NULL'")),
            Value::Null,
            Value::Text(String::from("NULL")),
            Value::Text(String::from("'TRUE'")),
            Value::Text(String::from("TRUE")),
            Value::Text(String::from("'5'")),
            Value::Text(String::from("5")),
            Value::Text(String::from("'it''s'")),
        ]
    );
}

#[test]
fn show_create_table_returns_canonical_roundtrippable_sql() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY)",
    );
    execute(
        &mut database,
        "CREATE TABLE widgets (\
            id INTEGER PRIMARY KEY AUTO_INCREMENT, \
            email TEXT NOT NULL UNIQUE DEFAULT 'it''s', \
            active BOOLEAN DEFAULT FALSE, \
            note TEXT DEFAULT NULL, \
            minimum INTEGER DEFAULT -9223372036854775808, \
            parent_id INTEGER REFERENCES parents(id) ON UPDATE CASCADE ON DELETE SET NULL, \
            code TEXT, \
            CHECK ((active = TRUE OR note IS NULL) \
                AND email != 'can''t' \
                AND minimum IN (-9223372036854775808, 0) \
                AND code LIKE 'a\\_\\%\\\\%')\
        )",
    );
    execute(
        &mut database,
        "INSERT INTO widgets (email, code) VALUES ('live', 'a_%\\tail')",
    );
    let before = database.as_str().to_owned();

    let result = row_set(&mut database, "sHoW CrEaTe TaBlE Widgets;");
    assert_eq!(database.as_str(), before);
    assert_eq!(result.columns().len(), 2);
    assert_eq!(result.columns()[0].label(), "table_name");
    assert_eq!(result.columns()[1].label(), "create_statement");
    assert!(result.columns().iter().all(|column| {
        column.origin().table() == "information_schema.tables"
            && column.data_type() == DataType::Text
            && !column.nullable()
    }));
    assert_eq!(result.columns()[0].origin().column(), "table_name");
    assert_eq!(result.columns()[1].origin().column(), "create_statement");

    let expected = "CREATE TABLE widgets (\
        id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, \
        email TEXT NOT NULL UNIQUE DEFAULT 'it''s', \
        active BOOLEAN DEFAULT FALSE, \
        note TEXT DEFAULT NULL, \
        minimum INTEGER DEFAULT -9223372036854775808, \
        parent_id INTEGER REFERENCES parents(id) ON DELETE SET NULL ON UPDATE CASCADE, \
        code TEXT, \
        CHECK (((active = TRUE OR note IS NULL) \
            AND email != 'can''t' \
            AND minimum IN (-9223372036854775808, 0) \
            AND code LIKE 'a\\_\\%\\\\%'))\
    )";
    assert_eq!(
        result.into_rows(),
        vec![vec![
            Value::Text(String::from("widgets")),
            Value::Text(String::from(expected)),
        ]]
    );

    let mut recreated = Database::new();
    execute(
        &mut recreated,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY)",
    );
    execute(&mut recreated, expected);
    assert_eq!(
        row_set(&mut recreated, "SHOW CREATE TABLE widgets").into_rows(),
        vec![vec![
            Value::Text(String::from("widgets")),
            Value::Text(String::from(expected)),
        ]]
    );
}

#[test]
fn metadata_statements_use_existing_unknown_table_diagnostic() {
    let mut database = Database::new();
    for sql in ["DESCRIBE missing", "SHOW CREATE TABLE missing"] {
        assert!(matches!(
            database.execute(sql),
            Err(Error::Schema(message)) if message == "unknown table \"missing\""
        ));
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

    for sql in ["SHOW TABLES", "DESCRIBE first", "SHOW CREATE TABLE first"] {
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
}

fn describe_row(
    name: &str,
    data_type: &str,
    nullable: bool,
    primary_key: bool,
    unique: bool,
    default: Value,
    auto_increment: bool,
) -> Vec<Value> {
    vec![
        Value::Text(String::from(name)),
        Value::Text(String::from(data_type)),
        Value::Boolean(nullable),
        Value::Boolean(primary_key),
        Value::Boolean(unique),
        default,
        Value::Boolean(auto_increment),
    ]
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
