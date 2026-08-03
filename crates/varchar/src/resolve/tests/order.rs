use super::{catalog, select_statement};
use crate::Error;
use crate::resolve::select;

#[test]
fn order_terms_resolve_against_sources_not_projection() {
    let catalog = catalog("V2;~S|parents|id:I:!|created:I:!;~S|children|parent_id:I:!|name:T:!;");
    let statement = select_statement(
        "SELECT children.name FROM parents \
         JOIN children ON parents.id = children.parent_id \
         ORDER BY parents.created DESC, children.name, parents.created DESC",
    );

    let resolved = select(&catalog, &statement, 2, 0, usize::MAX).expect("ORDER BY resolves");
    assert_eq!(resolved.projection.len(), 1);
    assert_eq!(resolved.order_by.len(), 3);
    assert_eq!(
        (
            resolved.order_by[0].column.source,
            resolved.order_by[0].column.column,
            resolved.order_by[0].descending,
        ),
        (0, 1, true)
    );
    assert_eq!(
        (
            resolved.order_by[1].column.source,
            resolved.order_by[1].column.column,
            resolved.order_by[1].descending,
        ),
        (1, 1, false)
    );
    assert_eq!(resolved.order_by[0], resolved.order_by[2]);
}

#[test]
fn ambiguous_and_non_source_order_columns_are_schema_errors() {
    let catalog = catalog("V2;~S|left_|id:I:!;~S|right_|id:I:!;");

    for (sql, expected) in [
        (
            "SELECT left_.id FROM left_ JOIN right_ ON left_.id = right_.id ORDER BY id",
            "ambiguous column \"id\"; qualify it with a table name",
        ),
        (
            "SELECT left_.id FROM left_ JOIN right_ ON left_.id = right_.id ORDER BY alias.id",
            "unknown table qualifier \"alias\"",
        ),
        (
            "SELECT left_.id FROM left_ JOIN right_ ON left_.id = right_.id ORDER BY missing",
            "unknown column \"missing\"",
        ),
    ] {
        let statement = select_statement(sql);
        assert!(
            matches!(
                select(&catalog, &statement, 2, 0, usize::MAX),
                Err(Error::Schema(message)) if message == expected
            ),
            "unexpected resolution for {sql:?}"
        );
    }
}
