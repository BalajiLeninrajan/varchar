use super::{decode_row, decode_schema_record, row_record, row_records};
use crate::storage::RowLayout;
use crate::storage::budget::{
    WorkingBudget, reset_working_string_comparisons, working_string_comparisons,
};
use crate::{DataType, Error, SchemaColumn, Value};

#[test]
fn row_view_exposes_the_complete_envelope_and_absolute_range() {
    let encoded = "~R|people|I1|Tleft%00007Cright%00003Bdone;";
    let row = row_record(encoded, 17).expect("valid row");

    assert_eq!(row.range(), 17..17 + encoded.len());
    assert_eq!(row.table(), "people");
    assert_eq!(
        row.cells().collect::<Vec<_>>(),
        vec!["I1", "Tleft%00007Cright%00003Bdone"]
    );

    let columns = [
        SchemaColumn {
            name: String::from("id"),
            data_type: DataType::Integer,
            nullable: false,
        },
        SchemaColumn {
            name: String::from("body"),
            data_type: DataType::Text,
            nullable: false,
        },
    ];
    assert_eq!(
        decode_row(
            encoded,
            RowLayout {
                table: "people",
                columns: &columns,
            },
        )
        .expect("escaped row decodes"),
        vec![
            Value::Integer(1),
            Value::Text(String::from("left|right;done")),
        ]
    );
}

#[test]
fn row_view_validates_the_complete_record_envelope() {
    assert_eq!(
        row_record("~R|people|I1;", 0).expect("valid row").table(),
        "people"
    );

    for malformed in [
        "~S|people|id:I:!;",
        "~R|People|I1;",
        "~R|people;",
        "~R|people|I1",
    ] {
        assert!(row_record(malformed, 0).is_err(), "accepted {malformed:?}");
    }
}

#[test]
fn row_iterator_starts_at_the_catalog_row_offset() {
    let schema = "~S|items|id:I:!;";
    let first = "~R|items|I1;";
    let second = "~R|items|I2;";
    let blob = format!("V2;{schema}{first}{second}");
    let row_start = "V2;".len() + schema.len();
    let rows = row_records(&blob, row_start)
        .collect::<crate::Result<Vec<_>>>()
        .expect("row suffix parses");

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].range(), row_start..row_start + first.len());
    assert_eq!(
        rows[1].range(),
        row_start + first.len()..row_start + first.len() + second.len()
    );
}

#[test]
fn row_iterator_is_empty_when_the_row_suffix_is_empty() {
    let blob = "V2;~S|items|id:I:!;";

    assert!(row_records(blob, blob.len()).next().is_none());
}

#[test]
fn row_iterator_rejects_a_non_row_at_its_start_offset() {
    let blob = "V2;~S|items|id:I:!;";
    let error = row_records(blob, 3)
        .next()
        .expect("schema record is present")
        .expect_err("schema is not a row");

    assert!(matches!(
        error,
        Error::CorruptStorage { offset: 3, message }
            if message == "expected a row record"
    ));
}

#[test]
fn wide_schema_duplicate_detection_uses_indexed_comparisons() {
    const COLUMN_COUNT: usize = 4_096;

    let mut record = String::from("~S|wide");
    for column in 0..COLUMN_COUNT {
        record.push_str(&format!("|c{column}:T:?"));
    }
    record.push(';');

    let mut budget = WorkingBudget::new(usize::MAX);
    reset_working_string_comparisons();
    let schema = decode_schema_record(&record, 0, &mut budget).expect("wide schema decodes");
    let (insert_comparisons, lookup_comparisons) = working_string_comparisons();

    assert_eq!(schema.columns.len(), COLUMN_COUNT);
    assert_eq!(lookup_comparisons, 0);
    assert!(
        insert_comparisons <= COLUMN_COUNT * 16,
        "{COLUMN_COUNT} distinct columns required {insert_comparisons} name comparisons"
    );
}

#[test]
fn indexed_column_detection_preserves_the_duplicate_diagnostic() {
    let mut budget = WorkingBudget::new(usize::MAX);
    let error = decode_schema_record("~S|items|a:I:!|b:I:!|c:I:!|d:I:!|a:T:?;", 17, &mut budget)
        .expect_err("duplicate column is rejected");

    assert!(matches!(
        error,
        Error::CorruptStorage { offset: 17, message }
            if message == "duplicate column name"
    ));
}
