use super::parse;
use crate::sql::ast::{
    ColumnDef, ColumnModifier, CreateElement, CreateTable, ForeignKeyReference, Predicate,
    PredicateOperator, Projection, Select, Statement, TableConstraint,
};
use crate::{DataType, Error, Value};

fn create_table(sql: &str) -> CreateTable {
    match parse(sql).expect("CREATE TABLE parses") {
        Statement::CreateTable(statement) => statement,
        other => panic!("expected CREATE TABLE, got {other:?}"),
    }
}

#[test]
fn parsing_produces_the_exact_normalized_ast() {
    assert_eq!(
        parse("SeLeCt Name, ID FROM Users WHERE Name LIKE 'a_%' AND ID != -7;")
            .expect("SELECT parses"),
        Statement::Select(Select {
            table: String::from("users"),
            projection: Projection::Columns(vec![String::from("name"), String::from("id"),]),
            predicates: vec![
                Predicate {
                    column: String::from("name"),
                    operator: PredicateOperator::Like(String::from("a_%")),
                },
                Predicate {
                    column: String::from("id"),
                    operator: PredicateOperator::NotEqual(Value::Integer(-7)),
                },
            ],
        })
    );
}

#[test]
fn unsupported_trailing_syntax_keeps_its_feature_and_span() {
    assert!(matches!(
        parse("SELECT * FROM t JOIN u"),
        Err(Error::Unsupported {
            ref feature,
            span_start: 16,
            span_end: 20,
        }) if feature == "joins"
    ));
}

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
