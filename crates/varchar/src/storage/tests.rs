use super::{StorageState, TableSchema, validate_and_catalog};
use crate::{DataType, SchemaColumn, Value};

#[test]
fn state_keeps_each_blob_with_its_derived_catalog() {
    let blob = String::from("V2;~S|items|id:I:!;~R|items|I1;");
    let state = StorageState::load(blob.clone()).expect("fixture is valid");
    let reconstructed = validate_and_catalog(state.as_str()).expect("stored blob remains valid");

    assert_eq!(state.catalog(), &reconstructed);
    assert!(state.catalog().table("items").is_some());
    assert_eq!(state.into_string(), blob);
}

#[test]
fn candidate_installs_key_metadata_and_a_matching_catalog_together() {
    let state = StorageState::empty();
    let schema = TableSchema {
        name: String::from("items"),
        columns: vec![SchemaColumn {
            name: String::from("id"),
            data_type: DataType::Integer,
            nullable: false,
        }],
        primary_key: Some(0),
        foreign_keys: Vec::new(),
    };
    let mut candidate = state.candidate(1024).expect("empty state fits");
    candidate
        .insert_schema_with_auto_increment(&schema, None)
        .expect("schema edit succeeds");
    candidate
        .append_row(schema.row_layout(), &[Value::Integer(1)])
        .expect("row edit succeeds");

    let next = candidate.finish().expect("candidate validates");
    let reconstructed =
        validate_and_catalog(next.as_str()).expect("finished candidate remains valid");

    assert_eq!(state.as_str(), "V2;");
    assert_eq!(next.catalog(), &reconstructed);
    assert_eq!(next.as_str(), "V2;~S|items|id:I:!;~P|items|id;~R|items|I1;");
}
