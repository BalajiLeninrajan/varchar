#![cfg(not(target_family = "wasm"))]

use varchar::{Database, Error};

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
fn v2_rejects_unique_metadata_at_the_record_start() {
    let blob = "V2;~S|t|value:I:?;~U|t|value;";
    assert_corrupt_at(blob, "~U|", "V3 metadata is invalid under a V2 header");
}

#[test]
fn unique_metadata_must_follow_defaults_and_precede_rows() {
    let after_row = "V3;~S|t|value:I:?;~R|t|I1;~U|t|value;";
    assert_corrupt_at(
        after_row,
        "~U|",
        "UNIQUE metadata appears after a row record",
    );

    let before_schema = "V3;~U|t|value;~S|t|value:I:?;";
    assert_corrupt_at(
        before_schema,
        "~U|",
        "UNIQUE metadata is outside its table's UNIQUE phase",
    );

    let default_after_unique = "V3;~S|t|a:I:?|b:I:?;~U|t|a;~D|t|b|I1;";
    assert_corrupt_at(
        default_after_unique,
        "~D|",
        "DEFAULT metadata is outside its table's DEFAULT phase",
    );

    let wrong_table = "V3;~S|t|value:I:?;~U|other|value;";
    assert_corrupt_at(
        wrong_table,
        "~U|",
        "UNIQUE metadata is outside its table's UNIQUE phase",
    );
}

#[test]
fn unique_metadata_is_canonical_and_never_duplicates_a_primary_key() {
    let descending = "V3;~S|t|a:I:?|b:I:?;~U|t|b;~U|t|a;";
    let second = descending.rfind("~U|").expect("second UNIQUE exists");
    assert_eq!(
        corruption(descending),
        (
            second,
            "UNIQUE metadata is duplicated or not in increasing column order".to_owned(),
        )
    );

    let duplicate = "V3;~S|t|a:I:?;~U|t|a;~U|t|a;";
    let second = duplicate.rfind("~U|").expect("second UNIQUE exists");
    assert_eq!(
        corruption(duplicate),
        (
            second,
            "UNIQUE metadata is duplicated or not in increasing column order".to_owned(),
        )
    );

    let unknown = "V3;~S|t|a:I:?;~U|t|missing;";
    assert_corrupt_at(
        unknown,
        "~U|",
        "UNIQUE for table \"t\" references unknown column \"missing\"",
    );

    let primary = "V3;~S|t|id:I:!;~P|t|id;~U|t|id;";
    assert_corrupt_at(
        primary,
        "~U|",
        "UNIQUE metadata must not duplicate a primary key",
    );
}

#[test]
fn malformed_unique_records_anchor_at_the_record_start() {
    for blob in [
        "V3;~S|t|value:I:?;~U|t;",
        "V3;~S|t|value:I:?;~U|t|value|extra;",
        "V3;~S|t|value:I:?;~U|t|bad-name;",
    ] {
        assert_corrupt_at(blob, "~U|", "malformed UNIQUE metadata");
    }
}

#[test]
fn nullable_unique_values_reload_with_multiple_nulls() {
    let blob = String::from("V3;~S|t|value:T:?;~U|t|value;~R|t|N;~R|t|N;");
    let database = Database::from_string(blob.clone()).expect("multiple NULL values are valid");
    assert_eq!(database.as_str(), blob);
}
