use super::{compile_scan, compile_select};
use crate::sql::{self, Statement};
use crate::storage::StorageState;
use crate::{Error, Limits};

#[test]
fn select_plans_borrow_sources_while_mutation_scans_own_their_layout() {
    let state = StorageState::load("V2;~S|items|id:I:!|name:T:?;~S|groups|id:I:!;".to_owned())
        .expect("fixture storage is valid");
    let catalog = state.catalog();
    let items = catalog.table("items").expect("items table exists");
    let groups = catalog.table("groups").expect("groups table exists");
    let Statement::Select(statement) =
        sql::parse("SELECT items.name FROM items JOIN groups ON items.id = groups.id")
            .expect("fixture SELECT parses")
    else {
        panic!("expected SELECT");
    };

    let select =
        compile_select(catalog, &statement, &Limits::default()).expect("SELECT plan compiles");
    assert!(std::ptr::eq(select.sources[0], items));
    assert!(std::ptr::eq(select.sources[1], groups));

    let scan = compile_scan(items, &[], &Limits::default()).expect("scan plan compiles");
    assert_eq!(scan.table, items.name);
    assert_eq!(scan.schema, items.columns);
}

#[test]
fn select_explanations_obey_the_query_output_budget() {
    let state =
        StorageState::load("V2;~S|items|id:I:!;".to_owned()).expect("fixture storage is valid");
    let Statement::Select(statement) =
        sql::parse("SELECT id FROM items").expect("fixture SELECT parses")
    else {
        panic!("expected SELECT");
    };
    let plan = compile_select(state.catalog(), &statement, &Limits::default())
        .expect("SELECT plan compiles");

    assert!(matches!(
        plan.into_explanation(0),
        Err(Error::ResourceLimit {
            resource: "query output bytes",
            limit: 0,
        })
    ));
}
