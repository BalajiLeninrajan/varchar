use super::{catalog, select_statement};
use crate::resolve::{ColumnLocation, select};
use crate::{Error, Resource};

#[test]
fn select_projection_resolves_qualified_stars_and_unambiguous_names() {
    let catalog = catalog("V2;~S|authors|id:I:!|name:T:!;~S|books|id:I:!|author_id:I:!|title:T:!;");
    let statement = select_statement(
        "SELECT name, books.*, authors.name FROM authors \
         JOIN books ON authors.id = books.author_id",
    );

    let resolved = select(&catalog, &statement, 2, 0, usize::MAX).expect("projection resolves");
    let locations = resolved
        .projection
        .iter()
        .map(|location| (location.source, location.column))
        .collect::<Vec<_>>();
    assert_eq!(locations, vec![(0, 1), (1, 0), (1, 1), (1, 2), (0, 1)]);
}

#[test]
fn expanded_select_projection_obeys_the_output_budget() {
    let catalog = catalog("V2;~S|t|id:I:!|name:T:!;");
    let statement = select_statement("SELECT t.*, t.* FROM t");
    let limit = std::mem::size_of::<ColumnLocation>() * 3;

    assert!(matches!(
        select(&catalog, &statement, 1, 0, limit),
        Err(Error::ResourceLimit {
            resource: Resource::QueryOutputBytes,
            limit: actual,
        }) if actual == limit
    ));
}

#[test]
fn select_semantic_errors_precede_the_projection_output_budget() {
    let catalog = catalog("V2;~S|t|id:I:!;~S|u|id:I:!;");
    let invalid_projection = select_statement("SELECT t.*, missing FROM t");
    assert!(matches!(
        select(&catalog, &invalid_projection, 1, 0, 0),
        Err(Error::Schema(ref message))
            if message == "unknown column \"missing\" in table \"t\""
    ));

    let invalid_join = select_statement("SELECT t.*, t.* FROM t JOIN u ON t.id = t.id");
    assert!(matches!(
        select(&catalog, &invalid_join, 2, 0, 0),
        Err(Error::Schema(ref message))
            if message == "JOIN for table \"u\" must connect it to an earlier table"
    ));

    let invalid_predicate = select_statement("SELECT t.* FROM t WHERE missing = 1");
    assert!(matches!(
        select(&catalog, &invalid_predicate, 1, 1, 0),
        Err(Error::Schema(ref message))
            if message == "unknown column \"missing\" in table \"t\""
    ));
}
