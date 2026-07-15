use super::{create_table, parse};
use crate::sql::ast::{
    ColumnDef, ColumnModifier, CreateElement, ForeignKeyReference, TableConstraint,
};
use crate::{DataType, Error};

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
fn rejects_composite_key_constraints_explicitly() {
    for sql in [
        "CREATE TABLE t (a INTEGER, b INTEGER, PRIMARY KEY (a, b))",
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
