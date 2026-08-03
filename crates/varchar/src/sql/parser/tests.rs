mod create;
mod expression;
mod metadata;
mod order_by;
mod pagination;

use super::parse;
use crate::sql::ast::{
    ColumnRef, CreateElement, CreateTable, Expression, ExpressionNode, Join, JoinCondition,
    Predicate, PredicateOperator, Projection, ProjectionItem, Select, Statement,
};
use crate::{Error, Value};

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
            where_clause: Some(Expression::new(vec![
                ExpressionNode::And { children: 2 },
                ExpressionNode::Predicate(Predicate {
                    column: column_ref(None, "name"),
                    operator: PredicateOperator::Like(String::from("a_%")),
                }),
                ExpressionNode::Predicate(Predicate {
                    column: column_ref(None, "id"),
                    operator: PredicateOperator::NotEqual(Value::Integer(-7)),
                }),
            ])),
            order_by: Vec::new(),
            limit: None,
            offset: None,
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
            where_clause: Some(Expression::new(vec![ExpressionNode::Predicate(
                Predicate {
                    column: column_ref(Some("books"), "title"),
                    operator: PredicateOperator::Like("R%".to_owned()),
                },
            )])),
            order_by: Vec::new(),
            limit: None,
            offset: None,
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
    let Some(expression) = &statement.where_clause else {
        panic!("expected WHERE expression");
    };
    let ExpressionNode::Predicate(predicate) = &expression.nodes()[0] else {
        panic!("expected predicate root");
    };
    assert_eq!(predicate.column, column_ref(Some("inner"), "on"));
}
