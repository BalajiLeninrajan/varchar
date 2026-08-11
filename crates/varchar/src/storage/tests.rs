mod state;

use super::budget::{reset_working_string_comparisons, working_limit, working_string_comparisons};
use super::decode::{blob_row_scans, reset_blob_row_scans};
use super::validate::validate_and_catalog;
use super::{StorageState, TableSchema};
use crate::{DataType, Error, Resource, SchemaColumn, Value};

#[test]
fn candidate_installs_key_metadata_and_a_matching_catalog_together() {
    let state = StorageState::empty();
    let schema = TableSchema {
        name: String::from("items"),
        columns: vec![SchemaColumn {
            name: String::from("id"),
            data_type: DataType::Integer,
            nullable: false,
            default: None,
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
    let (_, reconstructed) =
        validate_and_catalog(next.as_str(), usize::MAX).expect("finished candidate remains valid");

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
    let error = validate_and_catalog(&blob, usize::MAX).expect_err("duplicate key is rejected");
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
fn primary_key_index_preserves_exact_limit_loading() {
    let compact = "V2;~S|t|c0:I:!;~P|t|c0;~R|t|I0;~R|t|I1;~R|t|I2;";
    validate_and_catalog(compact, working_limit(compact.len()))
        .expect("a compact primary-key index fits its exact derived limit");

    let mut larger = String::from("V2;~S|t|id:T:!;~P|t|id;");
    for key in 0..=20 {
        larger.push_str(&format!("~R|t|Tk{key};"));
    }
    validate_and_catalog(&larger, working_limit(larger.len()))
        .expect("a larger primary-key index fits its exact derived limit");
}

#[test]
fn integrity_validation_never_sizes_an_index_with_its_own_blob_pass() {
    const ROW_COUNT: usize = 64;

    let mut keyed = String::from("V2;~S|items|id:I:!;~P|items|id;");
    for key in 0..ROW_COUNT {
        keyed.push_str(&format!("~R|items|I{key};"));
    }

    reset_blob_row_scans();
    validate_and_catalog(&keyed, usize::MAX).expect("a keyed fixture validates");
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
    validate_and_catalog(&referenced, usize::MAX).expect("a referenced fixture validates");
    assert_eq!(
        blob_row_scans(),
        2,
        "a referenced load adds only the foreign-key pass"
    );
}

/// The growth factor is bounded by the derived working limit rather than chosen for comfort.
///
/// This is the densest primary key a blob can carry: eight bytes of row per single-character
/// key, each indexed at `size_of::<&str>()` bytes, so an exactly sized index already spends
/// half of the four-times-database-size working limit and growth may only claim the other
/// half. The key count stops one past a growth step, where the overshoot is at its worst, and
/// the load still fits its exact derived limit. Growing by more than half would not: doubling
/// reserves 64 keys for these 33 and breaches the limit outright, so this fixture fails if the
/// growth factor is ever loosened.
#[test]
fn geometric_growth_stays_inside_the_derived_working_limit() {
    const PREFIX: &str = "V2;~S|t|c:T:!;~P|t|c;";
    const KEYS: &str = "abcdefghijklmnopqrstuvwxyz0123456";

    let mut blob = String::from(PREFIX);
    for key in KEYS.chars() {
        blob.push_str(&format!("~R|t|T{key};"));
    }
    assert_eq!(blob.len(), PREFIX.len() + KEYS.len() * 8);

    validate_and_catalog(&blob, working_limit(blob.len()))
        .expect("the worst geometric overshoot still fits the exact derived working limit");

    assert!(matches!(
        validate_and_catalog(&blob, 128),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit: 128,
        })
    ));
}

#[test]
fn sorted_primary_index_preserves_row_order_diagnostics() {
    let prefix = "V2;~S|items|id:I:!;~P|items|id;~A|items|id|I1;";

    let mut earlier_duplicate = String::from(prefix);
    earlier_duplicate.push_str("~R|items|I1;");
    let duplicate_offset = earlier_duplicate.len();
    earlier_duplicate.push_str("~R|items|I1;~R|items|I2;");
    assert!(matches!(
        validate_and_catalog(&earlier_duplicate, usize::MAX),
        Err(Error::CorruptStorage { offset, message })
            if offset == duplicate_offset
                && message == "duplicate primary key in table \"items\""
    ));

    let mut earlier_high_water = String::from(prefix);
    let high_water_offset = earlier_high_water.len();
    earlier_high_water.push_str("~R|items|I2;~R|items|I1;~R|items|I1;");
    assert!(matches!(
        validate_and_catalog(&earlier_high_water, usize::MAX),
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
    validate_and_catalog(&blob, usize::MAX).expect("matching foreign keys validate");
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
