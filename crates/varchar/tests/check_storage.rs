#![cfg(not(target_family = "wasm"))]

use varchar::{Database, Error, Limits};

fn corruption(blob: &str) -> (usize, String) {
    match Database::from_string(blob.to_owned()) {
        Err(Error::CorruptStorage { offset, message }) => (offset, message),
        Err(error) => panic!("expected corrupt storage for {blob:?}, got {error:?}"),
        Ok(_) => panic!("unexpectedly loaded corrupt storage {blob:?}"),
    }
}

fn assert_corrupt_at(blob: &str, needle: &str, message: &str) {
    let expected_offset = blob.find(needle).expect("offending payload exists");
    assert_eq!(corruption(blob), (expected_offset, message.to_owned()));
}

#[test]
fn v2_and_metadata_phase_violations_anchor_at_the_offending_record() {
    let v2 = "V2;~S|t|value:I:?;~C|t|ISNULL|0;";
    assert_corrupt_at(v2, "~C|", "V3 metadata is invalid under a V2 header");

    let before_schema = "V3;~C|t|ISNULL|0;~S|t|value:I:?;";
    assert_corrupt_at(
        before_schema,
        "~C|",
        "CHECK metadata is outside its table's CHECK phase",
    );

    let wrong_table = "V3;~S|t|value:I:?;~C|other|ISNULL|0;";
    assert_corrupt_at(
        wrong_table,
        "~C|",
        "CHECK metadata is outside its table's CHECK phase",
    );

    let after_row = "V3;~S|t|value:I:?;~R|t|I1;~C|t|ISNULL|0;";
    assert_eq!(
        corruption(after_row),
        (
            after_row.rfind("~C|").expect("CHECK record exists"),
            "CHECK metadata appears after a row record".to_owned(),
        )
    );

    let unique_after_check = "V3;~S|t|value:I:?;~C|t|ISNULL|0;~U|t|value;";
    assert_corrupt_at(
        unique_after_check,
        "~U|",
        "UNIQUE metadata is outside its table's UNIQUE phase",
    );
}

#[test]
fn malformed_opcodes_counts_positions_and_types_report_payload_offsets() {
    let unknown = "V3;~S|t|value:I:?;~C|t|BAD;";
    assert_corrupt_at(unknown, "BAD", "unknown CHECK program opcode");

    let bad_arity = "V3;~S|t|value:I:?;~C|t|AND|1|ISNULL|0;";
    assert_corrupt_at(
        bad_arity,
        "1|ISNULL",
        "CHECK AND/OR nodes require at least two children",
    );

    let noncanonical_count = "V3;~S|t|value:I:?;~C|t|AND|02|ISNULL|0|NOTNULL|0;";
    assert_corrupt_at(noncanonical_count, "02", "invalid CHECK child count");

    let outside_column = "V3;~S|t|value:I:?;~C|t|ISNULL|1;";
    assert_corrupt_at(
        outside_column,
        "1;",
        "CHECK column position is outside its table",
    );

    let noncanonical_column = "V3;~S|t|value:I:?;~C|t|ISNULL|00;";
    assert_corrupt_at(noncanonical_column, "00", "invalid CHECK column position");
}

#[test]
fn many_check_records_load_with_linear_metadata_growth() {
    const CHECK_COUNT: usize = 4_096;

    let mut blob = String::from("V3;~S|linear_growth_table|value:I:?;");
    for _ in 0..CHECK_COUNT {
        blob.push_str("~C|linear_growth_table|ISNULL|0;");
    }
    let limits = Limits {
        max_database_bytes: blob.len(),
        max_predicates: CHECK_COUNT,
        ..Limits::default()
    };
    let database = Database::from_string_with_limits(blob.clone(), limits)
        .expect("many CHECK records grow retained metadata geometrically");
    assert_eq!(database.as_str(), blob);
}
