use super::{assert_error, catalog, select_statement};
use crate::ErrorCode;
use crate::resolve::select;

#[test]
fn select_rejects_ambiguous_columns_and_unknown_qualifiers() {
    let catalog = catalog("V2;~S|authors|id:I:!|name:T:!;~S|books|id:I:!|author_id:I:!|title:T:!;");

    let ambiguous =
        select_statement("SELECT id FROM authors JOIN books ON authors.id = books.author_id");
    assert_error(
        select(&catalog, &ambiguous, 2, 0, usize::MAX),
        ErrorCode::Schema,
        "ambiguous column \"id\"; qualify it with a table name",
    );

    let unknown_qualifier = select_statement(
        "SELECT missing.id FROM authors JOIN books ON authors.id = books.author_id",
    );
    assert_error(
        select(&catalog, &unknown_qualifier, 2, 0, usize::MAX),
        ErrorCode::Schema,
        "unknown table qualifier \"missing\"",
    );
}
