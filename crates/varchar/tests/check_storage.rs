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

fn assert_corrupt(blob: &str) {
    assert!(
        matches!(
            Database::from_string(blob.to_owned()),
            Err(Error::CorruptStorage { .. })
        ),
        "unexpectedly accepted {blob:?}"
    );
}

#[test]
fn v2_and_metadata_phase_violations_anchor_at_the_offending_record() {
    let v2 = "V2;~S|t|value:I:?;~C|t|GT|0|I0;";
    assert_corrupt_at(v2, "~C|", "V3 metadata is invalid under a V2 header");

    let before_schema = "V3;~C|t|GT|0|I0;~S|t|value:I:?;";
    assert_corrupt_at(
        before_schema,
        "~C|",
        "CHECK metadata is outside its table's CHECK phase",
    );

    let wrong_table = "V3;~S|t|value:I:?;~C|other|GT|0|I0;";
    assert_corrupt_at(
        wrong_table,
        "~C|",
        "CHECK metadata is outside its table's CHECK phase",
    );

    let after_row = "V3;~S|t|value:I:?;~R|t|I1;~C|t|GT|0|I0;";
    assert_eq!(
        corruption(after_row),
        (
            after_row.rfind("~C|").expect("CHECK record exists"),
            "CHECK metadata appears after a row record".to_owned(),
        )
    );

    let unique_after_check = "V3;~S|t|value:I:?;~C|t|GT|0|I0;~U|t|value;";
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

    let bad_arity = "V3;~S|t|value:I:?;~C|t|AND|1|EQ|0|I1;";
    assert_corrupt_at(
        bad_arity,
        "1|EQ",
        "CHECK AND/OR nodes require at least two children",
    );

    let noncanonical_count = "V3;~S|t|value:I:?;~C|t|AND|02|EQ|0|I1|EQ|0|I2;";
    assert_corrupt_at(noncanonical_count, "02", "invalid CHECK child count");

    let outside_column = "V3;~S|t|value:I:?;~C|t|EQ|1|I1;";
    assert_corrupt_at(
        outside_column,
        "1|I1",
        "CHECK column position is outside its table",
    );

    let noncanonical_column = "V3;~S|t|value:I:?;~C|t|EQ|00|I1;";
    assert_corrupt_at(noncanonical_column, "00", "invalid CHECK column position");

    let wrong_type = "V3;~S|t|value:I:?;~C|t|EQ|0|Tone;";
    assert_corrupt_at(
        wrong_type,
        "Tone",
        "cell type does not match INTEGER column",
    );

    let null_comparison = "V3;~S|t|value:I:?;~C|t|GE|0|N;";
    assert_corrupt_at(
        null_comparison,
        "N;",
        "CHECK comparison operands cannot be NULL",
    );

    let like_integer = "V3;~S|t|value:I:?;~C|t|LIKE|0|1|La;";
    assert_corrupt_at(like_integer, "LIKE", "CHECK LIKE requires a TEXT column");

    let empty_in = "V3;~S|t|value:I:?;~C|t|IN|0|0;";
    assert_corrupt_at(empty_in, "0;", "CHECK IN requires at least one item");

    let wrong_in_type = "V3;~S|t|value:I:?;~C|t|IN|0|2|I1|Tbad;";
    assert_corrupt_at(
        wrong_in_type,
        "Tbad",
        "cell type does not match INTEGER column",
    );
}

#[test]
fn malformed_like_and_in_payloads_are_rejected() {
    for blob in [
        "V3;~S|t|value:T:?;~C|t|LIKE|0|1|Q;",
        "V3;~S|t|value:T:?;~C|t|LIKE|0|1|L;",
        "V3;~S|t|value:T:?;~C|t|LIKE|0|1|Lab;",
        "V3;~S|t|value:T:?;~C|t|LIKE|0|1|L%0000ZZ;",
        "V3;~S|t|value:T:?;~C|t|LIKE|0|2|La;",
        "V3;~S|t|value:T:?;~C|t|LIKE|0|1|La|Lb;",
        "V3;~S|t|value:T:?;~C|t|IN|0|2|Tone;",
        "V3;~S|t|value:T:?;~C|t|IN|0|01|Tone;",
    ] {
        assert_corrupt(blob);
    }

    let unknown_atom = "V3;~S|t|value:T:?;~C|t|LIKE|0|1|Q;";
    assert_corrupt_at(unknown_atom, "Q;", "invalid CHECK LIKE atom");

    let empty_literal = "V3;~S|t|value:T:?;~C|t|LIKE|0|1|L;";
    assert_eq!(
        corruption(empty_literal),
        (
            empty_literal.find("L;").expect("empty literal exists") + 1,
            "empty CHECK LIKE literal atom".to_owned(),
        )
    );

    let multiple_scalars = "V3;~S|t|value:T:?;~C|t|LIKE|0|1|Lab;";
    assert_corrupt_at(
        multiple_scalars,
        "b;",
        "CHECK LIKE literal atom must encode one Unicode scalar",
    );

    let malformed_escape = "V3;~S|t|value:T:?;~C|t|LIKE|0|1|L%0000ZZ;";
    assert_corrupt_at(malformed_escape, "%", "malformed text escape");

    let unnecessary_escape = "V3;~S|t|value:T:?;~C|t|LIKE|0|1|L%000061;";
    assert_corrupt_at(
        unnecessary_escape,
        "%",
        "unnecessary noncanonical text escape",
    );

    let invalid_scalar = "V3;~S|t|value:T:?;~C|t|LIKE|0|1|L%00D800;";
    assert_corrupt_at(invalid_scalar, "%", "escape is not a Unicode scalar");

    let missing_atom = "V3;~S|t|value:T:?;~C|t|LIKE|0|2|La;";
    assert_eq!(
        corruption(missing_atom),
        (
            missing_atom.len() - 1,
            "CHECK LIKE ends before all atoms are encoded".to_owned(),
        )
    );

    let extra_atom = "V3;~S|t|value:T:?;~C|t|LIKE|0|1|La|Lb;";
    assert_eq!(
        corruption(extra_atom),
        (
            extra_atom.rfind("Lb;").expect("extra atom exists"),
            "CHECK program contains trailing nodes or fields".to_owned(),
        )
    );

    let valid_null = String::from("V3;~S|t|value:T:?;~C|t|IN|0|1|N;~R|t|Tanything;");
    let database = Database::from_string(valid_null.clone()).expect("NULL IN item is valid");
    assert_eq!(database.as_str(), valid_null);

    let escaped_literals = String::from("V3;~S|t|value:T:?;~C|t|LIKE|0|2|L%00007C|L%00003B;");
    let database = Database::from_string(escaped_literals.clone())
        .expect("escaped structural LIKE literals are canonical");
    assert_eq!(database.as_str(), escaped_literals);
}

#[test]
fn malformed_counts_are_corruption_before_storage_working_limits() {
    for blob in [
        "V3;~S|t|value:T:?;~C|t|LIKE|0|999999999|La;",
        "V3;~S|t|value:T:?;~C|t|IN|0|999999999|Tone;",
    ] {
        let limits = Limits {
            max_database_bytes: blob.len(),
            ..Limits::default()
        };
        assert!(
            matches!(
                Database::from_string_with_limits(blob.to_owned(), limits),
                Err(Error::CorruptStorage { .. })
            ),
            "malformed CHECK count was not reported as corruption for {blob:?}"
        );
    }
}

#[test]
fn incomplete_trailing_and_noncanonical_trees_are_rejected() {
    let incomplete = "V3;~S|t|value:I:?;~C|t|AND|2|EQ|0|I1;";
    let (offset, message) = corruption(incomplete);
    assert_eq!(
        offset,
        incomplete.find(';').unwrap_or(0).max(incomplete.len() - 1)
    );
    assert_eq!(
        message,
        "CHECK program ends before all children are encoded"
    );

    let trailing = "V3;~S|t|value:I:?;~C|t|EQ|0|I1|EQ|0|I2;";
    assert_eq!(
        corruption(trailing),
        (
            trailing.rfind("EQ|").expect("second root exists"),
            "CHECK program contains trailing nodes or fields".to_owned(),
        )
    );

    let nested = "V3;~S|t|value:I:?;~C|t|AND|2|AND|2|EQ|0|I1|EQ|0|I2|EQ|0|I3;";
    assert_eq!(
        corruption(nested),
        (
            nested.rfind("AND|2").expect("nested AND exists"),
            "CHECK program contains a noncanonical nested associative node".to_owned(),
        )
    );

    for blob in [
        "V3;~S|t|value:I:?;~C|t;",
        "V3;~S|t|value:I:?;~C|t|;",
        "V3;~S|t|value:I:?;~C|t|OR|0;",
        "V3;~S|t|value:I:?;~C|t|OR|2;",
        "V3;~S|t|value:I:?;~C|t|EQ|0;",
    ] {
        assert_corrupt(blob);
    }
}

#[test]
fn many_check_records_load_with_linear_metadata_growth() {
    const CHECK_COUNT: usize = 4_096;

    let mut blob = String::from("V3;~S|t|value:I:?;");
    for _ in 0..CHECK_COUNT {
        blob.push_str("~C|t|GE|0|I-9223372036854775808;");
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
