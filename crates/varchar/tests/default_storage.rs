#![cfg(not(target_family = "wasm"))]

use varchar::{Database, Error, Limits, Resource};

fn corruption(blob: &str) -> (usize, String) {
    match Database::from_string(blob.to_owned()) {
        Err(Error::CorruptStorage { offset, message }) => (offset, message),
        Err(error) => panic!("expected corrupt storage for {blob:?}, got {error:?}"),
        Ok(_) => panic!("unexpectedly loaded corrupt storage {blob:?}"),
    }
}

fn assert_corrupt_at(blob: &str, needle: &str, message: &str) {
    let expected_offset = blob.find(needle).expect("offending record exists");
    assert_eq!(corruption(blob), (expected_offset, message.to_owned()));
}

#[test]
fn v2_rejects_default_metadata_at_the_record_start() {
    let blob = "V2;~S|t|value:I:?;~D|t|value|I1;";
    assert_corrupt_at(blob, "~D|", "V3 metadata is invalid under a V2 header");
}

#[test]
fn default_metadata_must_stay_in_its_canonical_table_phase() {
    let after_row = "V3;~S|t|value:I:?;~R|t|I1;~D|t|value|I2;";
    assert_corrupt_at(
        after_row,
        "~D|",
        "DEFAULT metadata appears after a row record",
    );

    let before_schema = "V3;~D|t|value|I1;~S|t|value:I:?;";
    assert_corrupt_at(
        before_schema,
        "~D|",
        "DEFAULT metadata is outside its table's DEFAULT phase",
    );

    let after_default = "V3;~S|t|id:I:!|note:T:?;~P|t|id;~D|t|note|Tx;~A|t|id|I0;";
    assert_corrupt_at(
        after_default,
        "~A|",
        "auto-increment metadata must follow its table's primary and foreign keys",
    );

    let wrong_table = "V3;~S|t|value:I:?;~D|other|value|I1;";
    assert_corrupt_at(
        wrong_table,
        "~D|",
        "DEFAULT metadata is outside its table's DEFAULT phase",
    );
}

#[test]
fn default_metadata_is_unique_and_in_increasing_column_order() {
    let descending = "V3;~S|t|a:I:?|b:T:?;~D|t|b|Tx;~D|t|a|I1;";
    let second = descending.rfind("~D|").expect("second DEFAULT exists");
    assert_eq!(
        corruption(descending),
        (
            second,
            "DEFAULT metadata is duplicated or not in increasing column order".to_owned(),
        )
    );

    let duplicate = "V3;~S|t|a:I:?;~D|t|a|I1;~D|t|a|I2;";
    let second = duplicate.rfind("~D|").expect("second DEFAULT exists");
    assert_eq!(
        corruption(duplicate),
        (
            second,
            "DEFAULT metadata is duplicated or not in increasing column order".to_owned(),
        )
    );

    let unknown = "V3;~S|t|a:I:?;~D|t|missing|I1;";
    assert_corrupt_at(
        unknown,
        "~D|",
        "DEFAULT for table \"t\" references unknown column \"missing\"",
    );
}

#[test]
fn default_payloads_use_canonical_typed_cell_encoding() {
    for (blob, payload, message) in [
        (
            "V3;~S|t|value:I:?;~D|t|value|I01;",
            "01",
            "noncanonical INTEGER cell",
        ),
        (
            "V3;~S|t|value:I:?;~D|t|value|Twrong;",
            "Twrong",
            "cell type does not match INTEGER column",
        ),
        (
            "V3;~S|t|value:B:?;~D|t|value|B2;",
            "B2",
            "invalid BOOLEAN cell",
        ),
        (
            "V3;~S|t|value:T:?;~D|t|value|T%00abcd;",
            "%00abcd",
            "malformed text escape",
        ),
        (
            "V3;~S|t|value:I:!;~D|t|value|N;",
            "N;",
            "NULL stored in a NOT NULL column",
        ),
    ] {
        let expected_offset = blob.find(payload).expect("payload exists");
        assert_eq!(corruption(blob), (expected_offset, message.to_owned()));
    }

    let empty = "V3;~S|t|value:I:?;~D|t|value|;";
    assert_corrupt_at(empty, "~D|", "malformed DEFAULT metadata");
}

#[test]
fn auto_increment_columns_reject_persisted_default_metadata() {
    let blob = "V3;~S|t|id:I:!;~P|t|id;~A|t|id|I0;~D|t|id|I1;";
    assert_corrupt_at(
        blob,
        "~D|",
        "auto-increment columns cannot have DEFAULT metadata",
    );
}

#[test]
fn storage_working_exhaustion_is_a_resource_error_not_corruption() {
    let mut schema = String::from("V3;~S|wide");
    for index in 0..32 {
        schema.push_str(&format!("|c{index}:T:?"));
    }
    schema.push(';');
    Database::from_string(schema.clone()).expect("fixture is structurally valid");

    let limit = schema.len();
    let limits = Limits {
        max_database_bytes: limit,
        ..Limits::default()
    };
    assert!(matches!(
        Database::from_string_with_limits(schema, limits),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit: working,
        }) if working == limit.saturating_mul(4)
    ));
}
