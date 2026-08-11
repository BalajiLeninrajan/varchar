use super::{compile_scan, compile_select};
use crate::sql::{self, Statement};
use crate::storage::{StorageState, reset_row_layout_validations, row_layout_validations};
use crate::{Error, Limits, Resource};

#[test]
fn select_plans_borrow_sources_while_mutation_scans_own_their_layout() {
    let state = StorageState::load(
        "V2;~S|items|id:I:!|name:T:?;~S|groups|id:I:!;".to_owned(),
        usize::MAX,
    )
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

    let validated_items = catalog
        .validated_table("items")
        .expect("items table is catalog-validated");
    reset_row_layout_validations();
    let scan = compile_scan(validated_items, None, &Limits::default()).expect("scan plan compiles");
    assert_eq!(row_layout_validations(), 0);
    let layout = scan.row_layout();
    assert_eq!(layout.table, items.name);
    assert_eq!(layout.columns, items.columns);
    assert_eq!(scan.validated_row_layout().column_count(), 2);
}

#[test]
fn mutation_scan_layouts_omit_default_payloads() {
    let state = StorageState::load(
        "V3;~S|items|body:T:?;~D|items|body|Tlarge_default;".to_owned(),
        usize::MAX,
    )
    .expect("fixture storage is valid");
    let catalog = state.catalog();
    let items = catalog.table("items").expect("items table exists");
    assert!(items.columns[0].default.is_some());

    let table = catalog
        .validated_table("items")
        .expect("items table is catalog-validated");
    let scan = compile_scan(table, None, &Limits::default()).expect("scan plan compiles");

    let layout = scan.row_layout();
    assert_eq!(layout.table, "items");
    assert_eq!(layout.columns.len(), 1);
    assert!(layout.columns[0].default.is_none());
}

#[test]
fn mutation_scans_own_their_catalog_validated_layout() {
    let scan = {
        let state = StorageState::load("V2;~S|items|id:I:!|name:T:?;".to_owned(), usize::MAX)
            .expect("fixture storage is valid");
        let table = state
            .catalog()
            .validated_table("items")
            .expect("items table is catalog-validated");
        compile_scan(table, None, &Limits::default()).expect("scan plan compiles")
    };

    let layout = scan.row_layout();
    assert_eq!(layout.table, "items");
    assert_eq!(layout.columns.len(), 2);
    assert_eq!(scan.validated_row_layout().column_count(), 2);
}

#[test]
fn select_explanations_obey_the_query_output_budget() {
    let state = StorageState::load("V2;~S|items|id:I:!;".to_owned(), usize::MAX)
        .expect("fixture storage is valid");
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
            resource: Resource::QueryOutputBytes,
            limit: 0,
        })
    ));
}
