mod state;

use super::budget::{reset_working_string_comparisons, working_string_comparisons};
use super::decode::{blob_row_scans, reset_blob_row_scans};
use super::validate::validate_and_catalog;
use super::{ForeignKeyDeleteAction, ForeignKeyUpdateAction, StorageState, TableSchema};
use crate::limits::storage_working_limit;
use crate::{DataType, Database, Error, Limits, Resource, SchemaColumn, Value};

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
        unique_columns: Vec::new(),
        foreign_keys: Vec::new(),
        checks: Vec::new(),
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
    validate_and_catalog(compact, storage_working_limit(compact.len()))
        .expect("a compact primary-key index fits its exact derived limit");

    let mut larger = String::from("V2;~S|t|id:T:!;~P|t|id;");
    for key in 0..=20 {
        larger.push_str(&format!("~R|t|Tk{key};"));
    }
    validate_and_catalog(&larger, storage_working_limit(larger.len()))
        .expect("a larger primary-key index fits its exact derived limit");
}

/// The UNIQUE analogue of `primary_key_index_preserves_exact_limit_loading`.
///
/// UNIQUE validation grows its values from the same fill pass that reads them, so what it
/// charges has to stay inside the limit the database size derives. The single-column fixtures
/// pin the growth — the last one is well past the reservation steps — and the column-heavy ones
/// pin that declaring a UNIQUE column costs nothing on its own: every one of them stopped
/// loading when a vector per column charged its header for each declared column.
///
/// The growth factor's own ceiling is pinned by
/// `geometric_growth_stays_inside_the_derived_working_limit` rather than here. A UNIQUE value
/// costs four bytes against the twelve its cell contributes to the derived limit, so no UNIQUE
/// fixture can be made tight enough to fail if the factor were loosened, whereas a primary key
/// costs sixteen against a row's thirty-two.
#[test]
fn unique_index_preserves_exact_limit_loading() {
    let compact = "V3;~S|t|c0:T:?;~U|t|c0;~R|t|T0;~R|t|T1;~R|t|T2;";
    validate_and_catalog(compact, storage_working_limit(compact.len()))
        .expect("a compact UNIQUE index fits its exact derived limit");

    let mut larger = String::from("V3;~S|t|value:T:?;~U|t|value;");
    for value in 0..=20 {
        larger.push_str(&format!("~R|t|Tv{value};"));
    }
    validate_and_catalog(&larger, storage_working_limit(larger.len()))
        .expect("a larger UNIQUE index fits its exact derived limit");

    let mut grown = String::from("V3;~S|t|value:T:?;~U|t|value;");
    for value in 0..64 {
        grown.push_str(&format!("~R|t|Tvalue{value};"));
    }
    validate_and_catalog(&grown, storage_working_limit(grown.len()))
        .expect("a grown UNIQUE index fits its exact derived limit");

    for (columns, rows) in [
        (2, 3),
        (3, 3),
        (4, 3),
        (2, 6),
        (3, 6),
        (4, 6),
        (8, 6),
        (16, 6),
    ] {
        let mut blob = String::from("V3;~S|t");
        for column in 0..columns {
            blob.push_str(&format!("|c{column}:T:?"));
        }
        blob.push(';');
        for column in 0..columns {
            blob.push_str(&format!("~U|t|c{column};"));
        }
        for row in 0..rows {
            blob.push_str("~R|t");
            for column in 0..columns {
                blob.push_str(&format!("|Tv{}", row * columns + column));
            }
            blob.push(';');
        }
        validate_and_catalog(&blob, storage_working_limit(blob.len())).unwrap_or_else(|error| {
            panic!(
                "{columns} UNIQUE columns over {rows} rows exceeded their derived limit: {error}"
            )
        });
    }
}

/// Regression: these databases reopened through the public API at a limit that is exactly their
/// own length, and a fixed working cost per declared UNIQUE column closed them out of it.
///
/// The property only holds once each validation pass hands its temporary indexes back, which is
/// why it is pinned here rather than beside the UNIQUE index that motivated it.
#[test]
fn small_unique_databases_reopen_at_their_own_size() {
    for blob in [
        "V3;~S|t0|i:T:!|u:T:?;~P|t0|i;~U|t0|u;~R|t0|Ta|Ta;",
        "V3;~S|t|c0:T:?|c1:T:?;~U|t|c0;~U|t|c1;~R|t|Ta|Td;~R|t|Tb|Te;~R|t|Tc|Tf;",
        "V3;~S|t|c0:T:?|c1:T:?;~U|t|c0;~U|t|c1;~R|t|Ta|N;~R|t|Tb|N;~R|t|Tc|N;",
    ] {
        let limits = Limits {
            max_database_bytes: blob.len(),
            ..Limits::default()
        };
        Database::from_string_with_limits(blob.to_string(), limits).unwrap_or_else(|error| {
            panic!(
                "{blob:?} no longer reopens at {} bytes: {error}",
                blob.len()
            )
        });
    }
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

    let mut keyed_unique = String::from("V3;~S|items|value:T:?;~U|items|value;");
    for key in 0..ROW_COUNT {
        keyed_unique.push_str(&format!("~R|items|Tk{key};"));
    }

    reset_blob_row_scans();
    validate_and_catalog(&keyed_unique, usize::MAX).expect("a UNIQUE fixture validates");
    assert_eq!(
        blob_row_scans(),
        2,
        "a UNIQUE load fills its validation offsets in the pass that reads them"
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

    validate_and_catalog(&blob, storage_working_limit(blob.len()))
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
fn legacy_restrict_foreign_key_metadata_is_canonical_in_v2_and_v3() {
    for header in ["V2;", "V3;"] {
        let blob = format!(
            "{header}~S|parents|id:I:!;~P|parents|id;\
             ~S|children|parent_id:I:?;~F|children|parent_id|parents|id;"
        );
        let (_, catalog) =
            validate_and_catalog(&blob, usize::MAX).expect("legacy foreign-key metadata decodes");
        let foreign_key = &catalog
            .table("children")
            .expect("children schema exists")
            .foreign_keys[0];
        assert_eq!(foreign_key.on_delete, ForeignKeyDeleteAction::Restrict);
        assert_eq!(foreign_key.on_update, ForeignKeyUpdateAction::Restrict);
    }
}

#[test]
fn extended_foreign_key_action_metadata_requires_v3() {
    let prefix = "V2;~S|parents|id:I:!;~P|parents|id;~S|children|parent_id:I:?;";
    for actions in ["C|R", "N|R", "R|R", "R|C"] {
        let blob = format!("{prefix}~F|children|parent_id|parents|id|{actions};");
        let offset = blob.find("~F|").expect("foreign key exists");
        assert!(matches!(
            validate_and_catalog(&blob, usize::MAX),
            Err(Error::CorruptStorage { offset: actual, message })
                if actual == offset && message == "V3 metadata is invalid under a V2 header"
        ));
    }

    let blob = "V3;~S|parents|id:I:!;~P|parents|id;\
                ~S|children|cascading:I:?|nulling:I:?|updating:I:?;\
                ~F|children|cascading|parents|id|C|R;\
                ~F|children|nulling|parents|id|N|R;\
                ~F|children|updating|parents|id|R|C;";
    let (_, catalog) =
        validate_and_catalog(blob, usize::MAX).expect("V3 foreign-key actions decode");
    let foreign_keys = &catalog
        .table("children")
        .expect("children schema exists")
        .foreign_keys;
    assert_eq!(foreign_keys[0].on_delete, ForeignKeyDeleteAction::Cascade);
    assert_eq!(foreign_keys[1].on_delete, ForeignKeyDeleteAction::SetNull);
    assert_eq!(foreign_keys[2].on_update, ForeignKeyUpdateAction::Cascade);
}

#[test]
fn persisted_v3_foreign_key_actions_must_be_canonical_and_applicable() {
    let prefix = "V3;~S|parents|id:I:!;~P|parents|id;~S|children|parent_id:I:?;";
    let explicit_defaults = format!("{prefix}~F|children|parent_id|parents|id|R|R;");
    let offset = explicit_defaults.find("~F|").expect("foreign key exists");
    assert!(matches!(
        validate_and_catalog(&explicit_defaults, usize::MAX),
        Err(Error::CorruptStorage { offset: actual, message })
            if actual == offset
                && message == "explicit RESTRICT/RESTRICT foreign-key actions are noncanonical"
    ));

    let invalid_update_action = format!("{prefix}~F|children|parent_id|parents|id|R|N;");
    let offset = invalid_update_action
        .find("~F|")
        .expect("foreign key exists");
    assert!(matches!(
        validate_and_catalog(&invalid_update_action, usize::MAX),
        Err(Error::CorruptStorage { offset: actual, message })
            if actual == offset && message == "malformed foreign-key action metadata"
    ));

    let invalid_set_null = "V3;~S|parents|id:I:!;~P|parents|id;\
                            ~S|children|parent_id:I:!;\
                            ~F|children|parent_id|parents|id|N|R;";
    let offset = invalid_set_null.find("~F|").expect("foreign key exists");
    assert!(matches!(
        validate_and_catalog(invalid_set_null, usize::MAX),
        Err(Error::CorruptStorage { offset: actual, message })
            if actual == offset
                && message
                    == "ON DELETE SET NULL requires nullable foreign-key column \"children\".\"parent_id\""
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
