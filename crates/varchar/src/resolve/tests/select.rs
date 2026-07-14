use super::{catalog, select_statement};
use crate::Value;
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
    let first = &resolved.predicates[0];
    assert_eq!(first.source, 1);
    assert!(matches!(
        &first.predicate,
        ResolvedPredicate::Like {
            column: 1,
            atoms
        } if atoms == &[LikeAtom::Literal('N'), LikeAtom::AnySequence]
    ));
    let second = &resolved.predicates[1];
    assert_eq!(second.source, 0);
    assert!(matches!(
        &second.predicate,
        ResolvedPredicate::Equal {
            column: 1,
            value: Value::Text(value)
        } if value == "Ada"
    ));
}
