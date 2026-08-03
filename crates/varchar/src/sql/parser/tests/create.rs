use super::{create_table, parse};
use crate::sql::ast::{
    ColumnDef, ColumnModifier, CreateElement, ForeignKeyReference, TableConstraint,
};
use crate::{DataType, Error, Value};

#[test]
fn parses_inline_primary_and_foreign_keys_in_either_modifier_order() {
    let statement = create_table(
        "CREATE TABLE children (\
            id INTEGER REFERENCES parents(id) PRIMARY KEY, \
            owner_id INTEGER NOT NULL REFERENCES owners(id), \
            note TEXT\
        )",
    );

    assert_eq!(
        statement.elements,
        vec![
            CreateElement::Column(ColumnDef {
                name: "id".to_owned(),
                data_type: DataType::Integer,
                modifiers: vec![
                    ColumnModifier::References(ForeignKeyReference {
                        table: "parents".to_owned(),
                        column: "id".to_owned(),
                    }),
                    ColumnModifier::PrimaryKey,
                ],
            }),
            CreateElement::Column(ColumnDef {
                name: "owner_id".to_owned(),
                data_type: DataType::Integer,
                modifiers: vec![
                    ColumnModifier::NotNull,
                    ColumnModifier::References(ForeignKeyReference {
                        table: "owners".to_owned(),
                        column: "id".to_owned(),
                    }),
                ],
            }),
            CreateElement::Column(ColumnDef {
                name: "note".to_owned(),
                data_type: DataType::Text,
                modifiers: Vec::new(),
            }),
        ]
    );
}

#[test]
fn preserves_table_elements_in_source_order() {
    let statement = create_table(
        "CREATE TABLE children (\
            PRIMARY KEY (id), \
            id INTEGER, \
            FOREIGN KEY (parent_id) REFERENCES parents(id), \
            parent_id INTEGER\
        )",
    );

    assert_eq!(
        statement.elements,
        vec![
            CreateElement::Constraint(TableConstraint::PrimaryKey("id".to_owned())),
            CreateElement::Column(ColumnDef {
                name: "id".to_owned(),
                data_type: DataType::Integer,
                modifiers: Vec::new(),
            }),
            CreateElement::Constraint(TableConstraint::ForeignKey {
                column: "parent_id".to_owned(),
                reference: ForeignKeyReference {
                    table: "parents".to_owned(),
                    column: "id".to_owned(),
                },
            }),
            CreateElement::Column(ColumnDef {
                name: "parent_id".to_owned(),
                data_type: DataType::Integer,
                modifiers: Vec::new(),
            }),
        ]
    );
}

#[test]
fn preserves_duplicate_declarations_for_semantic_resolution() {
    let statement = create_table(
        "CREATE TABLE items (\
            id INTEGER NOT NULL PRIMARY KEY REFERENCES parents(id) \
                NOT NULL PRIMARY KEY REFERENCES owners(id), \
            PRIMARY KEY (missing), \
            PRIMARY KEY (id)\
        )",
    );

    let CreateElement::Column(column) = &statement.elements[0] else {
        panic!("expected the first element to be a column");
    };
    assert_eq!(
        column.modifiers,
        vec![
            ColumnModifier::NotNull,
            ColumnModifier::PrimaryKey,
            ColumnModifier::References(ForeignKeyReference {
                table: "parents".to_owned(),
                column: "id".to_owned(),
            }),
            ColumnModifier::NotNull,
            ColumnModifier::PrimaryKey,
            ColumnModifier::References(ForeignKeyReference {
                table: "owners".to_owned(),
                column: "id".to_owned(),
            }),
        ]
    );
    assert_eq!(statement.elements.len(), 3);
}

#[test]
fn parses_literal_defaults_and_preserves_duplicates_for_resolution() {
    let statement = create_table(
        "CREATE TABLE settings (value TEXT DEFAULT NULL DEFAULT 'fallback', enabled BOOLEAN DEFAULT TRUE)",
    );
    let CreateElement::Column(value) = &statement.elements[0] else {
        panic!("expected a column");
    };
    assert_eq!(
        value.modifiers,
        vec![
            ColumnModifier::Default(Value::Null),
            ColumnModifier::Default(Value::Text("fallback".to_owned())),
        ]
    );
    let CreateElement::Column(enabled) = &statement.elements[1] else {
        panic!("expected a column");
    };
    assert_eq!(
        enabled.modifiers,
        vec![ColumnModifier::Default(Value::Boolean(true))]
    );
}

#[test]
fn default_is_reserved_and_cannot_be_used_as_an_identifier() {
    for sql in [
        "CREATE TABLE default (id INTEGER)",
        "CREATE TABLE t (default TEXT)",
        "INSERT INTO default (id) VALUES (1)",
        "SELECT default FROM t",
        "SELECT * FROM default",
        "SELECT * FROM t WHERE default = 1",
        "UPDATE default SET id = 1",
        "UPDATE t SET default = 1",
        "DELETE FROM default",
    ] {
        let span_start = sql.find("default").expect("fixture contains error marker");
        match parse(sql) {
            Err(Error::Parse {
                message,
                span_start: actual_start,
                span_end,
            }) => {
                assert_eq!(
                    message, "reserved keyword `DEFAULT` cannot be used as an identifier",
                    "message for {sql:?}"
                );
                assert_eq!(
                    (actual_start, span_end),
                    (span_start, span_start + "default".len()),
                    "span for {sql:?}"
                );
            }
            other => panic!("expected exact Parse error for {sql:?}, got {other:?}"),
        }
    }

    // The keyword still drives the column modifier it was reserved for.
    let statement = create_table("CREATE TABLE t (value TEXT DEFAULT 'fallback')");
    let CreateElement::Column(value) = &statement.elements[0] else {
        panic!("expected a column");
    };
    assert_eq!(
        value.modifiers,
        vec![ColumnModifier::Default(Value::Text("fallback".to_owned()))]
    );
}

#[test]
fn parses_inline_and_table_unique_declarations_in_source_order() {
    let statement = create_table(
        "CREATE TABLE accounts (email TEXT UNIQUE, UNIQUE (handle), handle TEXT UNIQUE)",
    );
    assert_eq!(
        statement.elements,
        vec![
            CreateElement::Column(ColumnDef {
                name: "email".to_owned(),
                data_type: DataType::Text,
                modifiers: vec![ColumnModifier::Unique],
            }),
            CreateElement::Constraint(TableConstraint::Unique("handle".to_owned())),
            CreateElement::Column(ColumnDef {
                name: "handle".to_owned(),
                data_type: DataType::Text,
                modifiers: vec![ColumnModifier::Unique],
            }),
        ]
    );
}

#[test]
fn unique_is_reserved_and_cannot_be_used_as_an_identifier() {
    for sql in [
        "CREATE TABLE unique (id INTEGER)",
        "CREATE TABLE t (unique INTEGER)",
        "INSERT INTO unique (id) VALUES (1)",
        "SELECT unique FROM t",
        "SELECT * FROM unique",
        "SELECT * FROM t WHERE unique = 1",
        "UPDATE unique SET id = 1",
        "UPDATE t SET unique = 1",
        "DELETE FROM unique",
    ] {
        let span_start = sql.find("unique").expect("fixture contains error marker");
        match parse(sql) {
            Err(Error::Parse {
                message,
                span_start: actual_start,
                span_end,
            }) => {
                assert_eq!(
                    message, "reserved keyword `UNIQUE` cannot be used as an identifier",
                    "message for {sql:?}"
                );
                assert_eq!(
                    (actual_start, span_end),
                    (span_start, span_start + "unique".len()),
                    "span for {sql:?}"
                );
            }
            other => panic!("expected exact Parse error for {sql:?}, got {other:?}"),
        }
    }

    // The keyword still parses where the grammar expects it.
    let statement = create_table("CREATE TABLE accounts (value TEXT UNIQUE, UNIQUE (value))");
    let CreateElement::Column(value) = &statement.elements[0] else {
        panic!("expected a column");
    };
    assert_eq!(value.modifiers, vec![ColumnModifier::Unique]);
    assert_eq!(
        statement.elements[1],
        CreateElement::Constraint(TableConstraint::Unique("value".to_owned()))
    );
}

#[test]
fn preserves_duplicate_unique_declarations_for_resolution() {
    let statement = create_table(
        "CREATE TABLE accounts (email TEXT UNIQUE UNIQUE, handle TEXT, UNIQUE (handle), UNIQUE (handle))",
    );
    let CreateElement::Column(email) = &statement.elements[0] else {
        panic!("expected a column");
    };
    assert_eq!(
        email.modifiers,
        vec![ColumnModifier::Unique, ColumnModifier::Unique]
    );
    assert_eq!(
        &statement.elements[2..],
        [
            CreateElement::Constraint(TableConstraint::Unique("handle".to_owned())),
            CreateElement::Constraint(TableConstraint::Unique("handle".to_owned())),
        ]
    );
}

#[test]
fn rejects_composite_key_constraints_explicitly() {
    for sql in [
        "CREATE TABLE t (a INTEGER, b INTEGER, PRIMARY KEY (a, b))",
        "CREATE TABLE t (a INTEGER, b INTEGER, UNIQUE (a, b))",
        "CREATE TABLE t (a INTEGER, b INTEGER, FOREIGN KEY (a, b) REFERENCES p(a))",
        "CREATE TABLE t (a INTEGER REFERENCES p(a, b))",
    ] {
        assert!(
            matches!(parse(sql), Err(Error::Unsupported { .. })),
            "expected composite constraint to be unsupported: {sql}"
        );
    }
}

#[test]
fn auto_increment_spellings_are_contextual_column_modifiers() {
    for modifier in ["AUTOINCREMENT", "AUTO_INCREMENT"] {
        let statement = create_table(&format!(
            "CREATE TABLE messages (id INTEGER PRIMARY KEY {modifier})"
        ));
        let CreateElement::Column(column) = &statement.elements[0] else {
            panic!("expected a column");
        };
        assert_eq!(
            column.modifiers,
            vec![ColumnModifier::PrimaryKey, ColumnModifier::AutoIncrement]
        );
    }

    let statement = create_table(
        "CREATE TABLE autoincrement (auto_increment INTEGER, value INTEGER PRIMARY KEY)",
    );
    assert_eq!(statement.table, "autoincrement");
    let CreateElement::Column(column) = &statement.elements[0] else {
        panic!("expected a column");
    };
    assert_eq!(column.name, "auto_increment");
    assert!(column.modifiers.is_empty());
}

#[test]
fn preserves_duplicate_auto_increment_modifiers_for_resolution() {
    let statement =
        create_table("CREATE TABLE ids (id INTEGER AUTOINCREMENT AUTO_INCREMENT AUTOINCREMENT)");
    let CreateElement::Column(column) = &statement.elements[0] else {
        panic!("expected a column");
    };
    assert_eq!(
        column.modifiers,
        vec![
            ColumnModifier::AutoIncrement,
            ColumnModifier::AutoIncrement,
            ColumnModifier::AutoIncrement,
        ]
    );
}
