mod state;

use super::decode::{blob_row_scans, reset_blob_row_scans};
use super::integrity::{reset_working_string_comparisons, working_string_comparisons};
use super::validate::validate_and_catalog;
use super::{StorageState, TableSchema};
use crate::{DataType, Error, SchemaColumn, Value};

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

#[test]
fn primary_key_validation_uses_indexed_duplicate_checks() {
    const ROW_COUNT: usize = 4_096;

    let mut blob = String::from("V2;~S|items|id:I:!;~P|items|id;");
    for key in 0..ROW_COUNT {
        blob.push_str(&format!("~R|items|I{key};"));
    }
    let duplicate_offset = blob.len();
    blob.push_str("~R|items|I0;");

    reset_working_string_comparisons();
    let error = validate_and_catalog(&blob).expect_err("duplicate key is rejected");
    let (insert_comparisons, lookup_comparisons) = working_string_comparisons();

    assert!(matches!(
        error,
        Error::CorruptStorage { offset, message }
            if offset == duplicate_offset && message == "duplicate primary key in table \"items\""
    ));
    assert_eq!(lookup_comparisons, 0);
    assert!(
        insert_comparisons <= ROW_COUNT * 16,
        "{ROW_COUNT} distinct keys required {insert_comparisons} duplicate comparisons"
    );
}

#[test]
fn integrity_validation_never_sizes_an_index_with_its_own_blob_pass() {
    const ROW_COUNT: usize = 64;

    let mut keyed = String::from("V2;~S|items|id:I:!;~P|items|id;");
    for key in 0..ROW_COUNT {
        keyed.push_str(&format!("~R|items|I{key};"));
    }

    reset_blob_row_scans();
    validate_and_catalog(&keyed).expect("a keyed fixture validates");
    assert_eq!(
        blob_row_scans(),
        1,
        "a keyed load fills its primary index in one pass"
    );

    let mut referenced = String::from(
        "V2;~S|parents|id:I:!;~P|parents|id;\
         ~S|children|id:I:!|parent_id:I:!;~P|children|id;\
         ~F|children|parent_id|parents|id;",
    );
    for key in 0..ROW_COUNT {
        referenced.push_str(&format!("~R|parents|I{key};"));
    }
    for key in 0..ROW_COUNT {
        referenced.push_str(&format!("~R|children|I{key}|I{key};"));
    }

    reset_blob_row_scans();
    validate_and_catalog(&referenced).expect("a referenced fixture validates");
    assert_eq!(
        blob_row_scans(),
        2,
        "a referenced load adds only the foreign-key pass"
    );
}

#[test]
fn sorted_primary_index_preserves_row_order_diagnostics() {
    let prefix = "V2;~S|items|id:I:!;~P|items|id;~A|items|id|I1;";

    let mut earlier_duplicate = String::from(prefix);
    earlier_duplicate.push_str("~R|items|I1;");
    let duplicate_offset = earlier_duplicate.len();
    earlier_duplicate.push_str("~R|items|I1;~R|items|I2;");
    assert!(matches!(
        validate_and_catalog(&earlier_duplicate),
        Err(Error::CorruptStorage { offset, message })
            if offset == duplicate_offset
                && message == "duplicate primary key in table \"items\""
    ));

    let mut earlier_high_water = String::from(prefix);
    let high_water_offset = earlier_high_water.len();
    earlier_high_water.push_str("~R|items|I2;~R|items|I1;~R|items|I1;");
    assert!(matches!(
        validate_and_catalog(&earlier_high_water),
        Err(Error::CorruptStorage { offset, message })
            if offset == high_water_offset
                && message
                    == "auto-increment high-water mark for table \"items\" is below a stored key"
    ));
}

#[test]
fn foreign_key_validation_uses_indexed_membership_checks() {
    const ROW_COUNT: usize = 4_096;

    let mut blob = String::from(
        "V2;~S|parents|id:I:!;~P|parents|id;\
         ~S|children|id:I:!|parent_id:I:!;~P|children|id;\
         ~F|children|parent_id|parents|id;",
    );
    for key in 0..ROW_COUNT {
        blob.push_str(&format!("~R|parents|I{key};"));
    }
    for key in 0..ROW_COUNT {
        blob.push_str(&format!("~R|children|I{key}|I{key};"));
    }

    reset_working_string_comparisons();
    validate_and_catalog(&blob).expect("matching foreign keys validate");
    let (insert_comparisons, lookup_comparisons) = working_string_comparisons();

    assert!(
        insert_comparisons <= ROW_COUNT * 2 * 16,
        "{} distinct keys required {insert_comparisons} duplicate comparisons",
        ROW_COUNT * 2
    );
    assert!(
        (ROW_COUNT..=ROW_COUNT * 16).contains(&lookup_comparisons),
        "{ROW_COUNT} foreign keys required {lookup_comparisons} membership comparisons"
    );
}
