//! Whole-blob validation and reconstruction of the derived schema catalog.

use std::collections::BTreeMap;

use super::decode::{decode_schema_record, validate_row_record};
use super::format::{HEADER, RecordKind, corrupt, records};
use super::{Catalog, TableSchema};
use crate::Result;

/// Validate an entire blob and reconstruct its derived schema catalog.
pub(crate) fn validate_and_catalog(blob: &str) -> Result<Catalog> {
    if !blob.starts_with(HEADER) {
        return Err(corrupt(0, "expected canonical V1; header"));
    }

    let mut tables = BTreeMap::<String, TableSchema>::new();
    let mut row_start = blob.len();
    let mut saw_row = false;

    for record in records(blob) {
        let record = record?;
        match record.kind {
            RecordKind::Schema => {
                if saw_row {
                    return Err(corrupt(
                        record.range.start,
                        "schema record appears after a row record",
                    ));
                }
                let schema = decode_schema_record(record.text, record.range.start)?;
                if tables.contains_key(&schema.name) {
                    return Err(corrupt(record.range.start, "duplicate table schema"));
                }
                tables.insert(schema.name.clone(), schema);
            }
            RecordKind::Row => {
                if !saw_row {
                    row_start = record.range.start;
                    saw_row = true;
                }
                validate_row_record(record.text, record.range.start, &tables)?;
            }
            RecordKind::Unknown => {
                return Err(corrupt(record.range.start, "unknown record tag"));
            }
        }
    }

    Ok(Catalog { tables, row_start })
}
