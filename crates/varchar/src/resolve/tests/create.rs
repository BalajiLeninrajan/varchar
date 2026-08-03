use crate::Error;
use crate::resolve::create_schema;
use crate::sql::{self, Statement};
use crate::storage::{Catalog, ForeignKey, StorageState};

fn create_table(sql: &str) -> crate::sql::CreateTable {
    let Statement::CreateTable(statement) = sql::parse(sql).expect("statement parses") else {
        panic!("expected CREATE TABLE");
    };
    statement
}

fn keyed_parent_catalog() -> Catalog {
    StorageState::load(
        String::from("V2;~S|parents|id:I:!|code:I:?|label:T:?;~P|parents|id;"),
        usize::MAX,
    )
    .expect("parent catalog is valid")
    .catalog()
    .clone()
}

#[test]
fn create_schema_normalizes_inline_and_table_key_metadata() {
    for sql in [
        "CREATE TABLE children (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parents(id))",
        "CREATE TABLE children (id INTEGER, parent_id INTEGER, PRIMARY KEY (id), FOREIGN KEY (parent_id) REFERENCES parents(id))",
        "CREATE TABLE children (PRIMARY KEY (id), FOREIGN KEY (parent_id) REFERENCES parents(id), id INTEGER, parent_id INTEGER)",
    ] {
        let resolved =
            create_schema(&keyed_parent_catalog(), create_table(sql)).expect("schema resolves");
        assert_eq!(resolved.auto_increment, None);
        let schema = resolved.schema;
        assert_eq!(schema.primary_key, Some(0));
        assert!(!schema.columns[0].nullable);
        assert_eq!(
            schema.foreign_keys,
            vec![ForeignKey {
                column: 1,
                referenced_table: String::from("parents"),
                referenced_column: String::from("id"),
            }]
        );
    }
}

#[test]
fn create_schema_owns_table_constraint_policy() {
    for (sql, expected) in [
        (
            "CREATE TABLE items (id INTEGER, PRIMARY KEY (missing))",
            "PRIMARY KEY references unknown column \"missing\" in table \"items\"",
        ),
        (
            "CREATE TABLE items (id INTEGER, FOREIGN KEY (missing) REFERENCES parents(id))",
            "FOREIGN KEY references unknown column \"missing\" in table \"items\"",
        ),
        (
            "CREATE TABLE items (id INTEGER PRIMARY KEY, PRIMARY KEY (id))",
            "duplicate PRIMARY KEY declaration for column \"id\"",
        ),
        (
            "CREATE TABLE items (id INTEGER PRIMARY KEY, other INTEGER, PRIMARY KEY (other))",
            "table \"items\" may have only one PRIMARY KEY column",
        ),
        (
            "CREATE TABLE items (id INTEGER, parent_id INTEGER REFERENCES parents(id), FOREIGN KEY (parent_id) REFERENCES parents(id))",
            "duplicate FOREIGN KEY declaration for column \"parent_id\"",
        ),
    ] {
        assert!(matches!(
            create_schema(&Catalog::empty(), create_table(sql)),
            Err(Error::Schema(ref message)) if message == expected
        ));
    }
}

#[test]
fn create_schema_owns_column_shape_and_modifier_policy() {
    for (sql, expected) in [
        (
            "CREATE TABLE items (missing INTEGER, id INTEGER, id TEXT)",
            "duplicate column name \"id\"",
        ),
        (
            "CREATE TABLE items (id INTEGER NOT NULL NOT NULL)",
            "duplicate NOT NULL declaration for column \"id\"",
        ),
        (
            "CREATE TABLE items (id INTEGER PRIMARY KEY PRIMARY KEY)",
            "duplicate PRIMARY KEY declaration for column \"id\"",
        ),
        (
            "CREATE TABLE items (id INTEGER REFERENCES parents(id) REFERENCES parents(id))",
            "duplicate REFERENCES declaration for column \"id\"",
        ),
        (
            "CREATE TABLE items (PRIMARY KEY (missing))",
            "table must contain at least one column",
        ),
    ] {
        assert!(matches!(
            create_schema(&keyed_parent_catalog(), create_table(sql)),
            Err(Error::Schema(ref message)) if message == expected
        ));
    }
}

#[test]
fn duplicate_columns_precede_declaration_errors_but_declarations_keep_source_order() {
    let duplicate_column =
        create_table("CREATE TABLE items (PRIMARY KEY (missing), id INTEGER, id INTEGER)");
    assert!(matches!(
        create_schema(&Catalog::empty(), duplicate_column),
        Err(Error::Schema(ref message)) if message == "duplicate column name \"id\""
    ));

    let declarations =
        create_table("CREATE TABLE items (id INTEGER NOT NULL NOT NULL PRIMARY KEY PRIMARY KEY)");
    assert!(matches!(
        create_schema(&Catalog::empty(), declarations),
        Err(Error::Schema(ref message))
            if message == "duplicate NOT NULL declaration for column \"id\""
    ));

    let interleaved = create_table(
        "CREATE TABLE items (FOREIGN KEY (missing) REFERENCES parents(id), id INTEGER NOT NULL NOT NULL)",
    );
    assert!(matches!(
        create_schema(&keyed_parent_catalog(), interleaved),
        Err(Error::Schema(ref message))
            if message == "FOREIGN KEY references unknown column \"missing\" in table \"items\""
    ));
}

#[test]
fn create_schema_resolves_foreign_key_targets_before_storage() {
    for (sql, expected) in [
        (
            "CREATE TABLE children (parent_id INTEGER REFERENCES missing(id))",
            "foreign key references unknown or later table \"missing\"",
        ),
        (
            "CREATE TABLE children (parent_id INTEGER REFERENCES parents(missing))",
            "foreign key target \"parents\".\"missing\" is not its table's primary key",
        ),
        (
            "CREATE TABLE children (parent_id INTEGER REFERENCES parents(code))",
            "foreign key target \"parents\".\"code\" is not its table's primary key",
        ),
        (
            "CREATE TABLE children (parent_id TEXT REFERENCES parents(id))",
            "foreign-key columns \"children\".\"parent_id\" and \"parents\".\"id\" have different types",
        ),
    ] {
        assert!(matches!(
            create_schema(&keyed_parent_catalog(), create_table(sql)),
            Err(Error::Schema(ref message)) if message == expected
        ));
    }

    let source_order = create_table(
        "CREATE TABLE children (first INTEGER REFERENCES missing_first(id), second INTEGER REFERENCES missing_second(id))",
    );
    assert!(matches!(
        create_schema(&Catalog::empty(), source_order),
        Err(Error::Schema(ref message))
            if message == "foreign key references unknown or later table \"missing_first\""
    ));
}

#[test]
fn self_referential_foreign_keys_use_the_finished_local_primary_key() {
    let resolved = create_schema(
        &Catalog::empty(),
        create_table(
            "CREATE TABLE nodes (parent_id INTEGER REFERENCES nodes(id), id INTEGER, PRIMARY KEY (id))",
        ),
    )
    .expect("self reference resolves against the final local schema");
    assert_eq!(resolved.auto_increment, None);
    let schema = resolved.schema;

    assert_eq!(schema.primary_key, Some(1));
    assert_eq!(
        schema.foreign_keys,
        vec![ForeignKey {
            column: 0,
            referenced_table: String::from("nodes"),
            referenced_column: String::from("id"),
        }]
    );
}

#[test]
fn auto_increment_uses_the_finished_primary_key() {
    for sql in [
        "CREATE TABLE ids (id INTEGER AUTOINCREMENT PRIMARY KEY)",
        "CREATE TABLE ids (id INTEGER AUTOINCREMENT, PRIMARY KEY (id))",
        "CREATE TABLE ids (PRIMARY KEY (id), id INTEGER AUTO_INCREMENT)",
    ] {
        let resolved =
            create_schema(&Catalog::empty(), create_table(sql)).expect("schema resolves");
        assert_eq!(resolved.auto_increment, Some(0));
        assert_eq!(resolved.schema.primary_key, Some(0));
        assert!(!resolved.schema.columns[0].nullable);
    }
}

#[test]
fn auto_increment_duplicates_and_applicability_are_resolver_owned() {
    for (sql, expected) in [
        (
            "CREATE TABLE ids (id INTEGER PRIMARY KEY AUTOINCREMENT AUTO_INCREMENT)",
            "duplicate AUTOINCREMENT declaration for column \"id\"",
        ),
        (
            "CREATE TABLE ids (a INTEGER PRIMARY KEY AUTOINCREMENT, b INTEGER AUTOINCREMENT)",
            "table \"ids\" may have only one auto-increment column",
        ),
        (
            "CREATE TABLE ids (id TEXT PRIMARY KEY AUTOINCREMENT)",
            "auto-increment column \"ids\".\"id\" must be its INTEGER primary key",
        ),
        (
            "CREATE TABLE ids (id INTEGER AUTOINCREMENT)",
            "auto-increment column \"ids\".\"id\" must be its INTEGER primary key",
        ),
    ] {
        assert!(matches!(
            create_schema(&Catalog::empty(), create_table(sql)),
            Err(Error::Schema(ref message)) if message == expected
        ));
    }
}

#[test]
fn auto_increment_declaration_and_applicability_errors_have_stable_precedence() {
    let duplicate_auto = create_table(
        "CREATE TABLE ids (id INTEGER AUTOINCREMENT AUTO_INCREMENT PRIMARY KEY PRIMARY KEY)",
    );
    assert!(matches!(
        create_schema(&Catalog::empty(), duplicate_auto),
        Err(Error::Schema(ref message))
            if message == "duplicate AUTOINCREMENT declaration for column \"id\""
    ));

    let duplicate_primary = create_table(
        "CREATE TABLE ids (id INTEGER PRIMARY KEY PRIMARY KEY AUTOINCREMENT AUTO_INCREMENT)",
    );
    assert!(matches!(
        create_schema(&Catalog::empty(), duplicate_primary),
        Err(Error::Schema(ref message))
            if message == "duplicate PRIMARY KEY declaration for column \"id\""
    ));

    let invalid_foreign_key_before_applicability = create_table(
        "CREATE TABLE ids (id TEXT PRIMARY KEY AUTOINCREMENT, parent TEXT REFERENCES missing(id))",
    );
    assert!(matches!(
        create_schema(&Catalog::empty(), invalid_foreign_key_before_applicability),
        Err(Error::Schema(ref message))
            if message == "foreign key references unknown or later table \"missing\""
    ));
}
