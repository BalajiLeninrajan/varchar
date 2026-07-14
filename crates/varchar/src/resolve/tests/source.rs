use super::{assert_error, assert_resource_error, catalog, select_statement};
use crate::resolve::select;
use crate::{ErrorCode, Resource};

#[test]
fn repeated_select_sources_are_rejected_during_resolution() {
    let catalog = catalog("V2;~S|nodes|id:I:!|parent_id:I:?;");
    let statement =
        select_statement("SELECT nodes.id FROM nodes JOIN nodes ON nodes.parent_id = nodes.id");

    assert_error(
        select(&catalog, &statement, 4, 4, usize::MAX),
        ErrorCode::Schema,
        "table \"nodes\" appears more than once in a SELECT",
    );
}

#[test]
fn select_source_limit_is_enforced() {
    let catalog = catalog("V2;~S|t|id:I:!|note:T:!;");
    let statement = select_statement("SELECT id FROM t WHERE id = 1 AND note = 'one'");

    assert_resource_error(
        select(&catalog, &statement, 0, 2, usize::MAX),
        Resource::JoinSources,
        0,
    );
}
