use std::cell::Cell;

use super::{encode_row_from, encoded_row_len_from, with_validated_row_encoder};
use crate::storage::{RowLayout, validate_row_layout};
use crate::{DataType, Error, SchemaColumn, Value};

fn column(name: &str, data_type: DataType) -> SchemaColumn {
    SchemaColumn {
        name: String::from(name),
        data_type,
        nullable: false,
        default: None,
    }
}

fn assert_stale_measurement(error: Error) {
    assert!(matches!(
        error,
        Error::Capacity {
            operation: "encoding a row from a stale measurement",
        }
    ));
}

#[test]
fn raw_row_layout_errors_precede_width_and_value_access() {
    let invalid_column = column("bad name", DataType::Integer);
    let duplicate_columns = vec![
        column("id", DataType::Integer),
        column("id", DataType::Integer),
    ];
    let invalid_before_duplicate = vec![
        column("bad name", DataType::Integer),
        column("id", DataType::Integer),
        column("id", DataType::Integer),
    ];
    let duplicate_before_invalid = vec![
        column("id", DataType::Integer),
        column("id", DataType::Integer),
        column("bad name", DataType::Integer),
    ];
    let cases = [
        (
            RowLayout {
                table: "bad name",
                columns: &[],
            },
            "invalid or noncanonical table name \"bad name\"",
        ),
        (
            RowLayout {
                table: "t",
                columns: &[],
            },
            "table must contain at least one column",
        ),
        (
            RowLayout {
                table: "t",
                columns: std::slice::from_ref(&invalid_column),
            },
            "invalid or noncanonical column name \"bad name\"",
        ),
        (
            RowLayout {
                table: "t",
                columns: &duplicate_columns,
            },
            "duplicate column name \"id\"",
        ),
        (
            RowLayout {
                table: "t",
                columns: &invalid_before_duplicate,
            },
            "invalid or noncanonical column name \"bad name\"",
        ),
        (
            RowLayout {
                table: "t",
                columns: &duplicate_before_invalid,
            },
            "duplicate column name \"id\"",
        ),
    ];

    for (layout, expected) in cases {
        let calls = Cell::new(0);
        let error = encoded_row_len_from(usize::MAX, layout, |_| {
            calls.set(calls.get() + 1);
            None
        })
        .expect_err("the malformed layout is rejected while measuring");
        assert!(matches!(error, Error::Schema(message) if message == expected));
        assert_eq!(calls.get(), 0);

        let calls = Cell::new(0);
        let error = encode_row_from(usize::MAX, layout, |_| {
            calls.set(calls.get() + 1);
            None
        })
        .expect_err("the malformed layout is rejected while encoding");
        assert!(matches!(error, Error::Schema(message) if message == expected));
        assert_eq!(calls.get(), 0);
    }
}

#[test]
fn measured_row_encoding_reads_each_value_once_and_preserves_bytes() {
    let columns = vec![
        column("id", DataType::Integer),
        column("body", DataType::Text),
    ];
    let layout = validate_row_layout(RowLayout {
        table: "items",
        columns: &columns,
    })
    .expect("valid layout");
    let values = [
        Value::Integer(7),
        Value::Text(String::from("%~|;\0\u{1f}\u{2028}\u{2029}é💾")),
    ];

    let measurement_calls = Cell::new(0);
    let encoding_calls = Cell::new(0);
    let (measured_len, encoded) = with_validated_row_encoder(layout, |encoder| {
        let measured = encoder
            .measure(values.len(), |column| {
                measurement_calls.set(measurement_calls.get() + 1);
                values.get(column)
            })
            .expect("row measures");
        assert_eq!(
            std::mem::size_of_val(&measured),
            std::mem::size_of::<usize>(),
            "each retained measurement stores only its exact length"
        );
        let measured_len = measured.encoded_len();
        let encoded = encoder
            .encode(measured, |column| {
                encoding_calls.set(encoding_calls.get() + 1);
                values.get(column)
            })
            .expect("measured row encodes");
        (measured_len, encoded)
    });

    assert_eq!(measurement_calls.get(), columns.len());
    assert_eq!(encoding_calls.get(), columns.len());
    assert_eq!(
        encoded,
        "~R|items|I7|T%000025%00007E%00007C%00003B%000000%00001F%002028%002029é💾;"
    );
    assert_eq!(measured_len, encoded.len());
}

#[test]
fn measured_short_then_encoded_longer_is_rejected_as_stale() {
    let columns = vec![column("body", DataType::Text)];
    let layout = validate_row_layout(RowLayout {
        table: "items",
        columns: &columns,
    })
    .expect("valid layout");
    let short = Value::Text(String::from("x"));
    let long = Value::Text(String::from("longer"));

    let error = with_validated_row_encoder(layout, |encoder| {
        let measured = encoder
            .measure(1, |_| Some(&short))
            .expect("short row measures");
        encoder.encode(measured, |_| Some(&long))
    })
    .expect_err("longer output cannot exceed the measured bound");

    assert_stale_measurement(error);
}

#[test]
fn measured_long_then_encoded_shorter_is_rejected_as_stale() {
    let columns = vec![column("body", DataType::Text)];
    let layout = validate_row_layout(RowLayout {
        table: "items",
        columns: &columns,
    })
    .expect("valid layout");
    let short = Value::Text(String::from("x"));
    let long = Value::Text(String::from("longer"));

    let error = with_validated_row_encoder(layout, |encoder| {
        let measured = encoder
            .measure(1, |_| Some(&long))
            .expect("long row measures");
        encoder.encode(measured, |_| Some(&short))
    })
    .expect_err("shorter output cannot underfill the measured bound");

    assert_stale_measurement(error);
}

#[test]
fn unequal_same_session_measurements_cannot_be_swapped() {
    let columns = vec![column("body", DataType::Text)];
    let layout = validate_row_layout(RowLayout {
        table: "items",
        columns: &columns,
    })
    .expect("valid layout");
    let short = Value::Text(String::from("x"));
    let long = Value::Text(String::from("longer"));

    let (longer_error, shorter_error) = with_validated_row_encoder(layout, |encoder| {
        let short_measurement = encoder
            .measure(1, |_| Some(&short))
            .expect("short row measures");
        let long_measurement = encoder
            .measure(1, |_| Some(&long))
            .expect("long row measures");
        let longer_error = encoder
            .encode(short_measurement, |_| Some(&long))
            .expect_err("a short measurement cannot encode a longer row");
        let shorter_error = encoder
            .encode(long_measurement, |_| Some(&short))
            .expect_err("a long measurement cannot encode a shorter row");
        (longer_error, shorter_error)
    });

    assert_stale_measurement(longer_error);
    assert_stale_measurement(shorter_error);
}

#[test]
fn measured_row_encoding_is_branded_to_its_validated_layout_session() {
    let first_columns = vec![column("id", DataType::Integer)];
    let second_columns = vec![column("body", DataType::Text)];
    let first = validate_row_layout(RowLayout {
        table: "first",
        columns: &first_columns,
    })
    .expect("first layout validates");
    let second = validate_row_layout(RowLayout {
        table: "second",
        columns: &second_columns,
    })
    .expect("second layout validates");
    let value = Value::Integer(1);

    assert_eq!(second.column_count(), 1);
    let encoded = with_validated_row_encoder(first, |encoder| {
        let measured = encoder
            .measure(1, |_| Some(&value))
            .expect("first layout measures");
        encoder
            .encode(measured, |_| Some(&value))
            .expect("measured row encodes")
    });
    assert_eq!(encoded, "~R|first|I1;");
}
