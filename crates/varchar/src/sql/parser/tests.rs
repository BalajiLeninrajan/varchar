use super::parse;
use crate::sql::ast::{
    ColumnDef, ColumnModifier, ColumnRef, CreateElement, CreateTable, ForeignKeyReference, Join,
    JoinCondition, Predicate, PredicateOperator, Projection, ProjectionItem, Select, Statement,
    TableConstraint,
};
use crate::{DataType, Error, Value};

fn create_table(sql: &str) -> CreateTable {
    match parse(sql).expect("CREATE TABLE parses") {
        Statement::CreateTable(statement) => statement,
        other => panic!("expected CREATE TABLE, got {other:?}"),
    }
}

fn select(sql: &str) -> Select {
    match parse(sql).expect("SELECT parses") {
        Statement::Select(statement) => statement,
        other => panic!("expected SELECT, got {other:?}"),
    }
}

fn column_ref(qualifier: Option<&str>, name: &str) -> ColumnRef {
    ColumnRef {
        qualifier: qualifier.map(str::to_owned),
        name: name.to_owned(),
    }
}

#[test]
fn parsing_produces_the_exact_normalized_ast() {
    assert_eq!(
        parse("SeLeCt Name, ID FROM Users WHERE Name LIKE 'a_%' AND ID != -7;")
            .expect("SELECT parses"),
        Statement::Select(Select {
            table: String::from("users"),
            joins: Vec::new(),
            projection: Projection::Items(vec![
                ProjectionItem::Column(column_ref(None, "name")),
                ProjectionItem::Column(column_ref(None, "id")),
            ]),
            predicates: vec![
                Predicate {
                    column: column_ref(None, "name"),
                    operator: PredicateOperator::Like(String::from("a_%")),
                },
                Predicate {
                    column: column_ref(None, "id"),
                    operator: PredicateOperator::NotEqual(Value::Integer(-7)),
                },
            ],
        })
    );
}

#[test]
fn unsupported_join_syntax_keeps_its_feature_and_span() {
    assert!(matches!(
        parse("SELECT * FROM t LEFT JOIN u ON t.id = u.id"),
        Err(Error::Unsupported {
            ref feature,
            span_start: 16,
            span_end: 20,
        }) if feature == "outer joins"
    ));
}

#[test]
fn parses_qualified_projection_inner_join_and_predicate_ast() {
    assert_eq!(
        select(
            "SELECT authors.name, books.* FROM authors INNER JOIN books \
             ON authors.id = books.author_id AND authors.kind = books.kind \
             WHERE books.title LIKE 'R%'",
        ),
        Select {
            table: "authors".to_owned(),
            joins: vec![Join {
                table: "books".to_owned(),
                conditions: vec![
                    JoinCondition {
                        left: column_ref(Some("authors"), "id"),
                        right: column_ref(Some("books"), "author_id"),
                    },
                    JoinCondition {
                        left: column_ref(Some("authors"), "kind"),
                        right: column_ref(Some("books"), "kind"),
                    },
                ],
            }],
            projection: Projection::Items(vec![
                ProjectionItem::Column(column_ref(Some("authors"), "name")),
                ProjectionItem::QualifiedAll("books".to_owned()),
            ]),
            predicates: vec![Predicate {
                column: column_ref(Some("books"), "title"),
                operator: PredicateOperator::Like("R%".to_owned()),
            }],
        }
    );
}

#[test]
fn preserves_repeated_join_sources_for_semantic_resolution() {
    let statement = select("SELECT nodes.id FROM nodes JOIN nodes ON nodes.parent_id = nodes.id");

    assert_eq!(statement.table, "nodes");
    assert_eq!(statement.joins.len(), 1);
    assert_eq!(statement.joins[0].table, "nodes");
}

#[test]
fn inner_and_on_remain_contextual_identifiers() {
    let statement = create_table("CREATE TABLE inner (on INTEGER)");
    assert_eq!(statement.table, "inner");
    let CreateElement::Column(column) = &statement.elements[0] else {
        panic!("expected a column");
    };
    assert_eq!(column.name, "on");

    let statement = select("SELECT inner.on FROM inner WHERE inner.on = 1");
    assert_eq!(
        statement.projection,
        Projection::Items(vec![ProjectionItem::Column(column_ref(
            Some("inner"),
            "on",
        ))])
    );
    assert_eq!(
        statement.predicates[0].column,
        column_ref(Some("inner"), "on")
    );
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
