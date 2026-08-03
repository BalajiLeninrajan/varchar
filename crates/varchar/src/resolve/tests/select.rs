use super::{catalog, select_statement};
use crate::Value;
use crate::expression::ProgramNode;
use crate::resolve::{LikeAtom, ResolvedPredicate, select};

#[test]
fn joined_select_resolution_tracks_sources_locations_and_predicates() {
    let catalog = catalog("V2;~S|authors|id:I:!|name:T:!;~S|books|author_id:I:!|title:T:!;");
    let statement = select_statement(
        "SELECT authors.name, books.title FROM authors \
         JOIN books ON authors.id = books.author_id \
         WHERE books.title LIKE 'N%' AND authors.name = 'Ada'",
    );

    let resolved = select(&catalog, &statement, 4, 4, usize::MAX).expect("SELECT resolves");
    assert_eq!(resolved.sources[0].name, "authors");
    assert_eq!(resolved.sources[1].name, "books");
    assert_eq!(
        (resolved.projection[0].source, resolved.projection[0].column),
        (0, 1)
    );
    assert_eq!(
        (resolved.projection[1].source, resolved.projection[1].column),
        (1, 1)
    );
    assert_eq!(resolved.joins[0].source, 1);
    assert_eq!(resolved.joins[0].conditions[0].left.source, 0);
    assert_eq!(resolved.joins[0].conditions[0].right.source, 1);

    let program = resolved.where_clause.as_ref().expect("WHERE resolves");
    assert!(matches!(
        program.nodes()[0],
        ProgramNode::And { children: 2 }
    ));
    assert!(matches!(
        &program.nodes()[1],
        ProgramNode::Predicate(ResolvedPredicate::Like { column, atoms })
            if (column.source, column.column) == (1, 1)
                && atoms == &[LikeAtom::Literal('N'), LikeAtom::AnySequence]
    ));
    assert!(matches!(
        &program.nodes()[2],
        ProgramNode::Predicate(ResolvedPredicate::Equal {
            column,
            value: Value::Text(value),
        }) if (column.source, column.column) == (0, 1) && value == "Ada"
    ));
}
