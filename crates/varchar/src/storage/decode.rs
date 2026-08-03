//! Canonical decoding and validation of storage records.

mod metadata;

use std::ops::Range;

pub(super) use metadata::{
    AutoIncrementMetadata, ForeignKeyMetadata, PrimaryKeyMetadata, decode_auto_increment_record,
    decode_foreign_key_record, decode_primary_key_record, decode_schema_record,
};

use super::format::{
    ROW_PREFIX, RecordKind, allocation_error, complete_record_body, corrupt, is_valid_identifier,
    records_from, scan_text,
};
use super::{RowLayout, TableSchema};
use crate::{DataType, Error, Result, SchemaColumn, Value};

/// A zero-copy view over a parsed V2 row envelope and validated table name.
///
/// Cell slices remain encoded so integrity validation can compare canonical
/// key values without allocating decoded rows.
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

    pub(super) fn cells(&self) -> std::str::Split<'a, char> {
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

pub(super) fn row_records(
    blob: &str,
    row_start: usize,
) -> impl Iterator<Item = Result<RowRecordRef<'_>>> {
    #[cfg(test)]
    record_blob_row_scan();
    records_from(blob, row_start).map(|record| {
        record.and_then(|record| {
            if record.kind != RecordKind::Row {
                return Err(corrupt(record.range.start, "expected a row record"));
            }
            row_record(record.text, record.range.start)
        })
    })
}

/// Decode a complete canonical row record for `schema`.
pub(crate) fn decode_row(record: &str, layout: RowLayout<'_>) -> Result<Vec<Value>> {
    decode_row_at(record, layout, 0)
}

pub(super) fn validate_row_record<'a>(
    record: &str,
    offset: usize,
    lookup_table: impl FnOnce(&str) -> Option<&'a TableSchema>,
) -> Result<()> {
    let body = complete_record_body(record, ROW_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    if !is_valid_identifier(table) {
        return Err(corrupt(
            offset + ROW_PREFIX.len(),
            "invalid or noncanonical table name",
        ));
    }
    let schema =
        lookup_table(table).ok_or_else(|| corrupt(offset, "row references an unknown table"))?;
    validate_row_at(record, schema.row_layout(), offset)
}

fn validate_row_at(record: &str, layout: RowLayout<'_>, offset: usize) -> Result<()> {
    let body = complete_record_body(record, ROW_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    if table != layout.table {
        return Err(corrupt(offset, "row table does not match its schema"));
    }
    let mut cell_offset = offset + ROW_PREFIX.len() + table.len() + 1;
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

fn decode_row_at(record: &str, layout: RowLayout<'_>, offset: usize) -> Result<Vec<Value>> {
    let body = complete_record_body(record, ROW_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    if table != layout.table {
        return Err(corrupt(offset, "row table does not match its schema"));
    }

    let mut values = Vec::new();
    values
        .try_reserve_exact(layout.columns.len())
        .map_err(|_| allocation_error("reserving decoded row cells"))?;
    let mut cell_offset = offset + ROW_PREFIX.len() + table.len() + 1;
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

fn validate_cell_at(encoded: &str, column: &SchemaColumn, offset: usize) -> Result<()> {
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

fn decode_cell_at(encoded: &str, column: &SchemaColumn, offset: usize) -> Result<Value> {
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

pub(super) fn decode_integer(payload: &str, offset: usize) -> Result<i64> {
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
        .map_err(|_| allocation_error("reserving decoded text"))?;
    scan_text(payload, offset, |character| decoded.push(character))?;
    Ok(decoded)
}

// Counts full-blob row scans so tests can pin the number of validation passes per load.
#[cfg(test)]
std::thread_local! {
    static BLOB_ROW_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_blob_row_scan() {
    BLOB_ROW_SCANS.with(|scans| scans.set(scans.get() + 1));
}

#[cfg(test)]
pub(super) fn reset_blob_row_scans() {
    BLOB_ROW_SCANS.with(|scans| scans.set(0));
}

#[cfg(test)]
pub(super) fn blob_row_scans() -> usize {
    BLOB_ROW_SCANS.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests;
