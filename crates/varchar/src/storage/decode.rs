//! Canonical decoding and validation of individual storage records.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use super::format::{
    AUTO_INCREMENT_PREFIX, FOREIGN_KEY_PREFIX, PRIMARY_KEY_PREFIX, ROW_PREFIX, RecordIter,
    RecordKind, SCHEMA_PREFIX, allocation_error, complete_record_body, corrupt,
    is_valid_identifier, records_from, scan_text,
};
use super::{RowLayout, TableSchema};
use crate::{Column, DataType, Error, Result, Value};

pub(super) struct PrimaryKeyMetadata<'a> {
    pub(super) table: &'a str,
    pub(super) column: &'a str,
}

pub(super) struct ForeignKeyMetadata<'a> {
    pub(super) table: &'a str,
    pub(super) column: &'a str,
    pub(super) referenced_table: &'a str,
    pub(super) referenced_column: &'a str,
}

pub(super) struct AutoIncrementMetadata<'a> {
    pub(super) table: &'a str,
    pub(super) column: &'a str,
    pub(super) last: i64,
}

/// A zero-copy view over a parsed V2 row envelope and validated table name.
///
/// Cell slices remain encoded; schema-aware decoding validates their width,
/// types, nullability, and canonical representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RowRecordRef<'a> {
    range: Range<usize>,
    table: &'a str,
    cells: &'a str,
}

impl<'a> RowRecordRef<'a> {
    pub(crate) fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub(crate) fn table(&self) -> &'a str {
        self.table
    }

    pub(crate) fn cells(&self) -> std::str::Split<'a, char> {
        self.cells.split('|')
    }
}

/// Parse one complete row envelope at its byte offset in the authoritative blob.
pub(crate) fn row_record(record: &str, offset: usize) -> Result<RowRecordRef<'_>> {
    let body = complete_record_body(record, ROW_PREFIX, offset)?;
    let (table, cells) = body
        .split_once('|')
        .ok_or_else(|| corrupt(offset, "row is missing its cell list"))?;
    if !is_valid_identifier(table) {
        return Err(corrupt(
            offset + ROW_PREFIX.len(),
            "invalid or noncanonical table name",
        ));
    }
    let end = offset
        .checked_add(record.len())
        .ok_or_else(|| corrupt(offset, "row range exceeds the database"))?;
    Ok(RowRecordRef {
        range: offset..end,
        table,
        cells,
    })
}

pub(super) struct RowRecordIter<'a> {
    records: RecordIter<'a>,
}

pub(super) fn row_records(blob: &str, row_start: usize) -> RowRecordIter<'_> {
    RowRecordIter {
        records: records_from(blob, row_start),
    }
}

impl<'a> Iterator for RowRecordIter<'a> {
    type Item = Result<RowRecordRef<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        let record = self.records.next()?;
        Some(record.and_then(|record| {
            if record.kind != RecordKind::Row {
                return Err(corrupt(record.range.start, "expected a row record"));
            }
            row_record(record.text, record.range.start)
        }))
    }
}

/// Decode a parsed row view for `schema`, validating every encoded cell.
pub(crate) fn decode_row(row: &RowRecordRef<'_>, layout: RowLayout<'_>) -> Result<Vec<Value>> {
    decode_row_view(row, layout)
}

pub(super) fn decode_schema_record(record: &str, offset: usize) -> Result<TableSchema> {
    let body = complete_record_body(record, SCHEMA_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields
        .next()
        .ok_or_else(|| corrupt(offset, "schema is missing a table name"))?;
    if !is_valid_identifier(table) {
        return Err(corrupt(offset + 3, "invalid or noncanonical table name"));
    }

    let mut columns = Vec::new();
    let column_count = body.bytes().filter(|byte| *byte == b'|').count();
    columns
        .try_reserve_exact(column_count)
        .map_err(|_| allocation_error("schema columns"))?;
    let mut names = BTreeSet::new();
    for field in fields {
        let mut parts = field.split(':');
        let name = parts.next().unwrap_or_default();
        let data_type = parts.next();
        let nullability = parts.next();
        if parts.next().is_some() || data_type.is_none() || nullability.is_none() {
            return Err(corrupt(offset, "malformed column descriptor"));
        }
        if !is_valid_identifier(name) {
            return Err(corrupt(offset, "invalid or noncanonical column name"));
        }
        if !names.insert(name) {
            return Err(corrupt(offset, "duplicate column name"));
        }
        let data_type = match data_type.unwrap() {
            "T" => DataType::Text,
            "I" => DataType::Integer,
            "B" => DataType::Boolean,
            _ => return Err(corrupt(offset, "unknown column type tag")),
        };
        let nullable = match nullability.unwrap() {
            "?" => true,
            "!" => false,
            _ => return Err(corrupt(offset, "invalid column nullability tag")),
        };
        columns.push(Column {
            name: name.to_owned(),
            data_type,
            nullable,
        });
    }
    if columns.is_empty() {
        return Err(corrupt(offset, "table must contain at least one column"));
    }

    Ok(TableSchema {
        name: table.to_owned(),
        columns,
        primary_key: None,
        foreign_keys: Vec::new(),
    })
}

pub(super) fn decode_primary_key_record(
    record: &str,
    offset: usize,
) -> Result<PrimaryKeyMetadata<'_>> {
    let body = complete_record_body(record, PRIMARY_KEY_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    let column = fields.next().unwrap_or_default();
    if fields.next().is_some() || !is_valid_identifier(table) || !is_valid_identifier(column) {
        return Err(corrupt(offset, "malformed primary-key metadata"));
    }
    Ok(PrimaryKeyMetadata { table, column })
}

pub(super) fn decode_foreign_key_record(
    record: &str,
    offset: usize,
) -> Result<ForeignKeyMetadata<'_>> {
    let body = complete_record_body(record, FOREIGN_KEY_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    let column = fields.next().unwrap_or_default();
    let referenced_table = fields.next().unwrap_or_default();
    let referenced_column = fields.next().unwrap_or_default();
    if fields.next().is_some()
        || !is_valid_identifier(table)
        || !is_valid_identifier(column)
        || !is_valid_identifier(referenced_table)
        || !is_valid_identifier(referenced_column)
    {
        return Err(corrupt(offset, "malformed foreign-key metadata"));
    }
    Ok(ForeignKeyMetadata {
        table,
        column,
        referenced_table,
        referenced_column,
    })
}

pub(super) fn decode_auto_increment_record(
    record: &str,
    offset: usize,
) -> Result<AutoIncrementMetadata<'_>> {
    let body = complete_record_body(record, AUTO_INCREMENT_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    let column = fields.next().unwrap_or_default();
    let encoded_last = fields.next().unwrap_or_default();
    if fields.next().is_some() || !is_valid_identifier(table) || !is_valid_identifier(column) {
        return Err(corrupt(offset, "malformed auto-increment metadata"));
    }
    let payload = encoded_last
        .strip_prefix('I')
        .ok_or_else(|| corrupt(offset, "auto-increment high-water mark must be an INTEGER"))?;
    let payload_offset = offset + AUTO_INCREMENT_PREFIX.len() + table.len() + 1 + column.len() + 2;
    let last = decode_integer(payload, payload_offset)?;
    Ok(AutoIncrementMetadata {
        table,
        column,
        last,
    })
}

pub(super) fn validate_row_record(
    record: &str,
    offset: usize,
    tables: &BTreeMap<String, TableSchema>,
) -> Result<()> {
    let row = row_record(record, offset)?;
    let row_offset = row.range().start;
    let schema = tables
        .get(row.table())
        .ok_or_else(|| corrupt(row_offset, "row references an unknown table"))?;
    validate_row_view(&row, schema.row_layout())
}

fn validate_row_view(row: &RowRecordRef<'_>, layout: RowLayout<'_>) -> Result<()> {
    let offset = row.range().start;
    if row.table() != layout.table {
        return Err(corrupt(offset, "row table does not match its schema"));
    }
    let mut fields = row.cells();
    let mut cell_offset = offset + ROW_PREFIX.len() + row.table().len() + 1;
    let mut cell_count = 0;
    for column in layout.columns {
        let Some(cell) = fields.next() else {
            return Err(row_width_error(offset, layout, cell_count));
        };
        validate_cell_at(cell, column, cell_offset)?;
        cell_count += 1;
        cell_offset += cell.len() + 1;
    }
    if fields.next().is_some() {
        cell_count += 1 + fields.count();
        return Err(row_width_error(offset, layout, cell_count));
    }
    Ok(())
}

fn decode_row_view(row: &RowRecordRef<'_>, layout: RowLayout<'_>) -> Result<Vec<Value>> {
    let offset = row.range().start;
    if row.table() != layout.table {
        return Err(corrupt(offset, "row table does not match its schema"));
    }
    let mut fields = row.cells();

    let mut values = Vec::new();
    values
        .try_reserve_exact(layout.columns.len())
        .map_err(|_| allocation_error("decoded row cells"))?;
    let mut cell_offset = offset + ROW_PREFIX.len() + row.table().len() + 1;
    for column in layout.columns {
        let Some(cell) = fields.next() else {
            return Err(row_width_error(offset, layout, values.len()));
        };
        values.push(decode_cell_at(cell, column, cell_offset)?);
        cell_offset += cell.len() + 1;
    }
    if fields.next().is_some() {
        let cell_count = values.len() + 1 + fields.count();
        return Err(row_width_error(offset, layout, cell_count));
    }
    Ok(values)
}

fn row_width_error(offset: usize, layout: RowLayout<'_>, actual: usize) -> Error {
    corrupt(
        offset,
        format!(
            "row for {:?} has {} cells, expected {}",
            layout.table,
            actual,
            layout.columns.len()
        ),
    )
}

fn validate_cell_at(encoded: &str, column: &Column, offset: usize) -> Result<()> {
    if encoded == "N" {
        return if column.nullable {
            Ok(())
        } else {
            Err(corrupt(offset, "NULL stored in a NOT NULL column"))
        };
    }

    match column.data_type {
        DataType::Text => {
            let payload = encoded
                .strip_prefix('T')
                .ok_or_else(|| corrupt(offset, "cell type does not match TEXT column"))?;
            scan_text(payload, offset + 1, |_| {})
        }
        DataType::Integer => {
            let payload = encoded
                .strip_prefix('I')
                .ok_or_else(|| corrupt(offset, "cell type does not match INTEGER column"))?;
            decode_integer(payload, offset + 1).map(|_| ())
        }
        DataType::Boolean => match encoded {
            "B0" | "B1" => Ok(()),
            _ => Err(corrupt(offset, "invalid BOOLEAN cell")),
        },
    }
}

fn decode_cell_at(encoded: &str, column: &Column, offset: usize) -> Result<Value> {
    if encoded == "N" {
        return if column.nullable {
            Ok(Value::Null)
        } else {
            Err(corrupt(offset, "NULL stored in a NOT NULL column"))
        };
    }

    match column.data_type {
        DataType::Text => {
            let payload = encoded
                .strip_prefix('T')
                .ok_or_else(|| corrupt(offset, "cell type does not match TEXT column"))?;
            decode_text(payload, offset + 1).map(Value::Text)
        }
        DataType::Integer => {
            let payload = encoded
                .strip_prefix('I')
                .ok_or_else(|| corrupt(offset, "cell type does not match INTEGER column"))?;
            decode_integer(payload, offset + 1).map(Value::Integer)
        }
        DataType::Boolean => match encoded {
            "B0" => Ok(Value::Boolean(false)),
            "B1" => Ok(Value::Boolean(true)),
            _ => Err(corrupt(offset, "invalid BOOLEAN cell")),
        },
    }
}

fn decode_integer(payload: &str, offset: usize) -> Result<i64> {
    let value: i64 = payload
        .parse()
        .map_err(|_| corrupt(offset, "invalid INTEGER cell"))?;
    if !is_canonical_integer(payload) {
        return Err(corrupt(offset, "noncanonical INTEGER cell"));
    }
    Ok(value)
}

fn is_canonical_integer(payload: &str) -> bool {
    if payload == "0" {
        return true;
    }
    let digits = payload.strip_prefix('-').unwrap_or(payload);
    let mut bytes = digits.bytes();
    bytes
        .next()
        .is_some_and(|byte| (b'1'..=b'9').contains(&byte))
        && bytes.all(|byte| byte.is_ascii_digit())
}

fn decode_text(payload: &str, offset: usize) -> Result<String> {
    let mut decoded = String::new();
    decoded
        .try_reserve(payload.len())
        .map_err(|_| allocation_error("decoded text bytes"))?;
    scan_text(payload, offset, |character| decoded.push(character))?;
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::{decode_row, row_record, row_records};
    use crate::storage::RowLayout;
    use crate::{Column, DataType, Error, Value};

    #[test]
    fn row_view_owns_the_complete_envelope_and_absolute_range() {
        let encoded = "~R|people|I1|Tleft%00007Cright%00003Bdone;";
        let row = row_record(encoded, 17).expect("valid row");

        assert_eq!(row.range(), 17..17 + encoded.len());
        assert_eq!(row.table(), "people");
        assert_eq!(
            row.cells().collect::<Vec<_>>(),
            vec!["I1", "Tleft%00007Cright%00003Bdone"]
        );

        let columns = [
            Column {
                name: String::from("id"),
                data_type: DataType::Integer,
                nullable: false,
            },
            Column {
                name: String::from("body"),
                data_type: DataType::Text,
                nullable: false,
            },
        ];
        assert_eq!(
            decode_row(
                &row,
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
    fn row_view_routes_prefix_like_table_names_exactly() {
        let user = row_record("~R|user|I1;", 0).expect("user row");
        let users = row_record("~R|users|I2;", 12).expect("users row");

        assert_eq!(user.table(), "user");
        assert_eq!(users.table(), "users");
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
}
