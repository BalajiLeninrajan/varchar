use super::{catalog, select_statement};
use crate::Error;
use crate::resolve::select;

#[test]
fn joins_must_connect_each_new_source_with_compatible_columns() {
    let catalog = catalog(
        "V2;~S|authors|id:I:!|name:T:!;~S|books|author_id:I:!|title:T:!;~S|reviews|book_id:I:!;",
    );

    let disconnected = select_statement(
        "SELECT * FROM authors \
         JOIN books ON authors.id = books.author_id \
         JOIN reviews ON authors.id = books.author_id",
    );
    assert!(matches!(
        select(&catalog, &disconnected, 3, 0, usize::MAX),
        Err(Error::Schema(ref message))
            if message == "JOIN for table \"reviews\" must connect it to an earlier table"
    ));

    let type_mismatch =
        select_statement("SELECT * FROM authors JOIN books ON authors.name = books.author_id");
    assert!(matches!(
        select(&catalog, &type_mismatch, 2, 0, usize::MAX),
        Err(Error::Type(ref message))
            if message
                == "JOIN columns \"authors\".\"name\" and \"books\".\"author_id\" have different types"
    ));
}
