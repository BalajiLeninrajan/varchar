use std::fmt::Write as _;

use crate::Error;
use crate::resolve::create::{default_validations, reset_default_validations};
use crate::resolve::create_schema;
use crate::sql::{self, Statement};
use crate::storage::{
    Catalog, ForeignKey, ForeignKeyDeleteAction, ForeignKeyUpdateAction, StorageState,
};

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
                on_delete: ForeignKeyDeleteAction::Restrict,
                on_update: ForeignKeyUpdateAction::Restrict,
            }]
        );
    }
}

#[test]
fn create_schema_preserves_foreign_key_actions_and_defaults() {
    let resolved = create_schema(
        &keyed_parent_catalog(),
        create_table(
            "CREATE TABLE children (cascade_id INTEGER NOT NULL REFERENCES parents(id) ON DELETE CASCADE ON UPDATE CASCADE, null_id INTEGER, default_id INTEGER REFERENCES parents(id), FOREIGN KEY (null_id) REFERENCES parents(id) ON DELETE SET NULL ON UPDATE RESTRICT)",
        ),
    )
    .expect("foreign-key actions resolve");

    assert_eq!(
        resolved.schema.foreign_keys,
        vec![
            ForeignKey {
                column: 0,
                referenced_table: String::from("parents"),
                referenced_column: String::from("id"),
                on_delete: ForeignKeyDeleteAction::Cascade,
                on_update: ForeignKeyUpdateAction::Cascade,
            },
            ForeignKey {
                column: 1,
                referenced_table: String::from("parents"),
                referenced_column: String::from("id"),
                on_delete: ForeignKeyDeleteAction::SetNull,
                on_update: ForeignKeyUpdateAction::Restrict,
            },
            ForeignKey {
                column: 2,
                referenced_table: String::from("parents"),
                referenced_column: String::from("id"),
                on_delete: ForeignKeyDeleteAction::Restrict,
                on_update: ForeignKeyUpdateAction::Restrict,
            },
        ]
    );
}

#[test]
fn on_delete_set_null_requires_a_nullable_local_column() {
    for sql in [
        "CREATE TABLE children (parent_id INTEGER NOT NULL REFERENCES parents(id) ON DELETE SET NULL)",
        "CREATE TABLE children (parent_id INTEGER REFERENCES parents(id) ON DELETE SET NULL NOT NULL)",
        "CREATE TABLE children (parent_id INTEGER NOT NULL, FOREIGN KEY (parent_id) REFERENCES parents(id) ON DELETE SET NULL)",
    ] {
        assert!(matches!(
            create_schema(&keyed_parent_catalog(), create_table(sql)),
            Err(Error::Schema(ref message))
                if message
                    == "ON DELETE SET NULL requires nullable foreign-key column \"children\".\"parent_id\""
        ));
    }
}

#[test]
fn foreign_key_action_diagnostics_follow_source_order() {
    for sql in [
        "CREATE TABLE children (parent_id INTEGER NOT NULL REFERENCES parents(id) ON DELETE SET NULL, value INTEGER UNIQUE UNIQUE)",
        "CREATE TABLE children (FOREIGN KEY (parent_id) REFERENCES parents(id) ON DELETE SET NULL, parent_id INTEGER NOT NULL, value INTEGER NOT NULL NOT NULL)",
    ] {
        assert!(matches!(
            create_schema(&keyed_parent_catalog(), create_table(sql)),
            Err(Error::Schema(ref message))
                if message
                    == "ON DELETE SET NULL requires nullable foreign-key column \"children\".\"parent_id\""
        ));
    }

    let action_before_default = create_table(
        "CREATE TABLE children (parent_id INTEGER NOT NULL REFERENCES parents(id) ON DELETE SET NULL, value INTEGER DEFAULT 'wrong')",
    );
    assert!(matches!(
        create_schema(&keyed_parent_catalog(), action_before_default),
        Err(Error::Schema(ref message))
            if message
                == "ON DELETE SET NULL requires nullable foreign-key column \"children\".\"parent_id\""
    ));

    let default_before_action = create_table(
        "CREATE TABLE children (value INTEGER DEFAULT 'wrong', parent_id INTEGER NOT NULL REFERENCES parents(id) ON DELETE SET NULL)",
    );
    assert!(matches!(
        create_schema(&keyed_parent_catalog(), default_before_action),
        Err(Error::Type(ref message)) if message == "column \"value\" expects INTEGER, got TEXT"
    ));

    let target_before_action = create_table(
        "CREATE TABLE children (parent_id INTEGER NOT NULL REFERENCES missing(id) ON DELETE SET NULL)",
    );
    assert!(matches!(
        create_schema(&Catalog::empty(), target_before_action),
        Err(Error::Schema(ref message))
            if message == "foreign key references unknown or later table \"missing\""
    ));
}

#[test]
fn wide_foreign_key_defaults_are_validated_once() {
    const COLUMN_COUNT: usize = 512;

    let mut sql = String::from("CREATE TABLE children (");
    for column in 0..COLUMN_COUNT {
        if column != 0 {
            sql.push_str(", ");
        }
        write!(
            sql,
            "c{column} INTEGER DEFAULT {column} REFERENCES parents(id)"
        )
        .expect("writing SQL to a String succeeds");
    }
    sql.push(')');

    reset_default_validations();
    create_schema(&keyed_parent_catalog(), create_table(&sql)).expect("wide schema resolves");
    assert_eq!(default_validations(), COLUMN_COUNT);
}

#[test]
fn create_schema_normalizes_single_column_unique_metadata() {
    let resolved = create_schema(
        &Catalog::empty(),
        create_table(
            "CREATE TABLE accounts (id INTEGER UNIQUE PRIMARY KEY, email TEXT UNIQUE, handle TEXT, UNIQUE (handle))",
        ),
    )
    .expect("UNIQUE declarations resolve");
    assert_eq!(resolved.schema.primary_key, Some(0));
    assert_eq!(resolved.schema.unique_columns, vec![1, 2]);
}

#[test]
fn duplicate_unique_declarations_are_schema_errors_even_on_primary_keys() {
    for sql in [
        "CREATE TABLE t (value TEXT UNIQUE UNIQUE)",
        "CREATE TABLE t (value TEXT UNIQUE, UNIQUE (value))",
        "CREATE TABLE t (value TEXT PRIMARY KEY UNIQUE, UNIQUE (value))",
    ] {
        assert!(matches!(
            create_schema(&Catalog::empty(), create_table(sql)),
            Err(Error::Schema(ref message))
                if message == "duplicate UNIQUE declaration for column \"value\""
        ));
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
            "CREATE TABLE items (id INTEGER, UNIQUE (missing))",
            "UNIQUE references unknown column \"missing\" in table \"items\"",
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
fn default_diagnostics_follow_source_order() {
    for sql in [
        "CREATE TABLE items (value INTEGER DEFAULT 'wrong' DEFAULT 1)",
        "CREATE TABLE items (value INTEGER DEFAULT 'wrong', parent_id INTEGER REFERENCES missing(id))",
        "CREATE TABLE items (value INTEGER DEFAULT 'wrong', id TEXT PRIMARY KEY AUTOINCREMENT)",
        "CREATE TABLE items (value INTEGER DEFAULT 'wrong', id INTEGER NOT NULL NOT NULL)",
        "CREATE TABLE items (value INTEGER DEFAULT 'wrong', id INTEGER PRIMARY KEY PRIMARY KEY)",
        "CREATE TABLE items (value INTEGER DEFAULT 'wrong', id INTEGER REFERENCES parents(id) REFERENCES parents(id))",
        "CREATE TABLE items (value INTEGER DEFAULT 'wrong' UNIQUE UNIQUE)",
        "CREATE TABLE items (value INTEGER DEFAULT 'wrong', UNIQUE (missing))",
        "CREATE TABLE items (value INTEGER DEFAULT 'wrong', id INTEGER, PRIMARY KEY (missing))",
        "CREATE TABLE items (value INTEGER DEFAULT 'wrong', id INTEGER, FOREIGN KEY (missing) REFERENCES parents(id))",
    ] {
        assert!(matches!(
            create_schema(&Catalog::empty(), create_table(sql)),
            Err(Error::Type(ref message))
                if message == "column \"value\" expects INTEGER, got TEXT"
        ));
    }

    for sql in [
        "CREATE TABLE items (value INTEGER NOT NULL DEFAULT NULL DEFAULT 1)",
        "CREATE TABLE items (value INTEGER NOT NULL DEFAULT NULL, parent_id INTEGER REFERENCES missing(id))",
        "CREATE TABLE items (value INTEGER NOT NULL DEFAULT NULL, id TEXT PRIMARY KEY AUTOINCREMENT)",
    ] {
        assert!(matches!(
            create_schema(&Catalog::empty(), create_table(sql)),
            Err(Error::Schema(ref message))
                if message == "DEFAULT NULL is invalid for NOT NULL column \"items\".\"value\""
        ));
    }

    let earlier_foreign_key = create_table(
        "CREATE TABLE items (parent_id INTEGER REFERENCES missing(id), value INTEGER DEFAULT 'wrong')",
    );
    assert!(matches!(
        create_schema(&Catalog::empty(), earlier_foreign_key),
        Err(Error::Schema(ref message))
            if message == "foreign key references unknown or later table \"missing\""
    ));

    let earlier_auto_increment = create_table(
        "CREATE TABLE items (id TEXT PRIMARY KEY AUTOINCREMENT, value INTEGER DEFAULT 'wrong')",
    );
    assert!(matches!(
        create_schema(&Catalog::empty(), earlier_auto_increment),
        Err(Error::Schema(ref message))
            if message == "auto-increment column \"items\".\"id\" must be its INTEGER primary key"
    ));

    for (sql, expected) in [
        (
            "CREATE TABLE items (value INTEGER UNIQUE UNIQUE DEFAULT 'wrong')",
            "duplicate UNIQUE declaration for column \"value\"",
        ),
        (
            "CREATE TABLE items (UNIQUE (missing), value INTEGER DEFAULT 'wrong')",
            "UNIQUE references unknown column \"missing\" in table \"items\"",
        ),
    ] {
        assert!(matches!(
            create_schema(&Catalog::empty(), create_table(sql)),
            Err(Error::Schema(ref message)) if message == expected
        ));
    }

    let activated_auto_increment = create_table(
        "CREATE TABLE items (id INTEGER PRIMARY KEY DEFAULT 1 REFERENCES parents(id) AUTO_INCREMENT, other INTEGER REFERENCES missing(id))",
    );
    assert!(matches!(
        create_schema(&keyed_parent_catalog(), activated_auto_increment),
        Err(Error::Schema(ref message))
            if message == "auto-increment column \"items\".\"id\" cannot have a DEFAULT"
    ));

    let resumed_defaults = create_table(
        "CREATE TABLE items (first INTEGER DEFAULT 1 REFERENCES parents(id), bad INTEGER DEFAULT 'wrong' REFERENCES missing(id))",
    );
    assert!(matches!(
        create_schema(&keyed_parent_catalog(), resumed_defaults),
        Err(Error::Type(ref message)) if message == "column \"bad\" expects INTEGER, got TEXT"
    ));

    let successful_foreign_key = create_table(
        "CREATE TABLE items (id TEXT PRIMARY KEY AUTOINCREMENT, value INTEGER DEFAULT 'wrong', parent_id INTEGER REFERENCES parents(id))",
    );
    assert!(matches!(
        create_schema(&keyed_parent_catalog(), successful_foreign_key),
        Err(Error::Schema(ref message))
            if message == "auto-increment column \"items\".\"id\" must be its INTEGER primary key"
    ));

    let failing_foreign_key = create_table(
        "CREATE TABLE items (id TEXT PRIMARY KEY AUTOINCREMENT, value INTEGER DEFAULT 'wrong', parent_id INTEGER REFERENCES missing(id))",
    );
    assert!(matches!(
        create_schema(&Catalog::empty(), failing_foreign_key),
        Err(Error::Type(ref message)) if message == "column \"value\" expects INTEGER, got TEXT"
    ));
}

#[test]
fn check_diagnostics_follow_source_order_against_default_and_declaration_errors() {
    let check_before_default =
        create_table("CREATE TABLE items (value INTEGER CHECK (missing = 0) DEFAULT 'wrong')");
    assert!(matches!(
        create_schema(&Catalog::empty(), check_before_default),
        Err(Error::Schema(ref message))
            if message == "CHECK references unknown column \"missing\" in table \"items\""
    ));

    let default_before_check =
        create_table("CREATE TABLE items (value INTEGER DEFAULT 'wrong' CHECK (missing = 0))");
    assert!(matches!(
        create_schema(&Catalog::empty(), default_before_check),
        Err(Error::Type(ref message)) if message == "column \"value\" expects INTEGER, got TEXT"
    ));

    let check_before_local_error =
        create_table("CREATE TABLE items (value INTEGER CHECK (missing = 0) UNIQUE UNIQUE)");
    assert!(matches!(
        create_schema(&Catalog::empty(), check_before_local_error),
        Err(Error::Schema(ref message))
            if message == "CHECK references unknown column \"missing\" in table \"items\""
    ));

    let local_error_before_check =
        create_table("CREATE TABLE items (value INTEGER UNIQUE UNIQUE CHECK (missing = 0))");
    assert!(matches!(
        create_schema(&Catalog::empty(), local_error_before_check),
        Err(Error::Schema(ref message))
            if message == "duplicate UNIQUE declaration for column \"value\""
    ));
}

#[test]
fn table_check_foreign_key_and_auto_increment_errors_keep_phase_order() {
    for (sql, expected) in [
        (
            "CREATE TABLE items (CHECK (missing = 0), parent_id INTEGER REFERENCES missing(id), id TEXT PRIMARY KEY AUTOINCREMENT)",
            "CHECK references unknown column \"missing\" in table \"items\"",
        ),
        (
            "CREATE TABLE items (parent_id INTEGER REFERENCES missing(id), CHECK (missing = 0), id TEXT PRIMARY KEY AUTOINCREMENT)",
            "foreign key references unknown or later table \"missing\"",
        ),
        (
            "CREATE TABLE items (id TEXT PRIMARY KEY AUTOINCREMENT, CHECK (missing = 0), parent_id INTEGER REFERENCES missing(id))",
            "CHECK references unknown column \"missing\" in table \"items\"",
        ),
        (
            "CREATE TABLE items (id TEXT PRIMARY KEY AUTOINCREMENT, parent_id INTEGER REFERENCES missing(id), CHECK (missing = 0))",
            "foreign key references unknown or later table \"missing\"",
        ),
        (
            "CREATE TABLE items (CHECK (missing = 0), id TEXT PRIMARY KEY AUTOINCREMENT, parent_id INTEGER REFERENCES parents(id))",
            "CHECK references unknown column \"missing\" in table \"items\"",
        ),
        (
            "CREATE TABLE items (id TEXT PRIMARY KEY AUTOINCREMENT, CHECK (missing = 0), parent_id INTEGER REFERENCES parents(id))",
            "auto-increment column \"items\".\"id\" must be its INTEGER primary key",
        ),
    ] {
        assert!(matches!(
            create_schema(&keyed_parent_catalog(), create_table(sql)),
            Err(Error::Schema(ref message)) if message == expected
        ));
    }
}

#[test]
fn check_resolution_uses_the_full_preflighted_column_namespace() {
    let resolved = create_schema(
        &Catalog::empty(),
        create_table("CREATE TABLE items (first INTEGER CHECK (later = 0), later INTEGER)"),
    )
    .expect("CHECK can reference a later local column");
    assert_eq!(resolved.schema.checks.len(), 1);

    let duplicate_column = create_table(
        "CREATE TABLE items (value INTEGER CHECK (missing = 0), value INTEGER DEFAULT 'wrong')",
    );
    assert!(matches!(
        create_schema(&Catalog::empty(), duplicate_column),
        Err(Error::Schema(ref message)) if message == "duplicate column name \"value\""
    ));
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
            on_delete: ForeignKeyDeleteAction::Restrict,
            on_update: ForeignKeyUpdateAction::Restrict,
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
