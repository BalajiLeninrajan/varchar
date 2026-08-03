//! Whole-blob validation and reconstruction of the derived schema catalog.

mod metadata;

use super::decode::{
    decode_auto_increment_record, decode_foreign_key_record, decode_primary_key_record,
    decode_schema_record, validate_row_record,
};
use super::format::{HEADER, RecordKind, corrupt, records};
use super::{Catalog, integrity};
use crate::{Error, Result};

use metadata::MetadataValidator;

#[derive(Clone, Copy)]
pub(super) enum ValidationMode {
    Persisted,
    Candidate,
}

/// Validate an authoritative blob and reconstruct its derived schema catalog.
pub(crate) fn validate_and_catalog(blob: &str) -> Result<Catalog> {
    validate_with_mode(blob, ValidationMode::Persisted)
}

/// Validate a database assembled by a SQL mutation.
///
/// Structural encoding failures remain corruption errors. Invalid key
/// definitions are schema errors, while row-level violations are constraints.
pub(crate) fn validate_candidate(blob: &str) -> Result<Catalog> {
    validate_with_mode(blob, ValidationMode::Candidate)
}

fn validate_with_mode(blob: &str, mode: ValidationMode) -> Result<Catalog> {
    if !blob.starts_with(HEADER) {
        return Err(corrupt(0, "expected canonical V2; header"));
    }

    let mut metadata = MetadataValidator::new();
    let mut row_start = blob.len();
    let mut saw_row = false;

    for record in records(blob) {
        let record = record?;
        match record.kind {
            RecordKind::Schema => {
                reject_after_rows(saw_row, record.range.start, "schema record")?;
                let schema = decode_schema_record(record.text, record.range.start)?;
                metadata.insert_schema(schema, record.range.start)?;
            }
            RecordKind::PrimaryKey => {
                reject_after_rows(saw_row, record.range.start, "primary-key metadata")?;
                let primary_key = decode_primary_key_record(record.text, record.range.start)?;
                metadata.apply_primary_key(primary_key, record.range.start, mode)?;
            }
            RecordKind::ForeignKey => {
                reject_after_rows(saw_row, record.range.start, "foreign-key metadata")?;
                let foreign_key = decode_foreign_key_record(record.text, record.range.start)?;
                metadata.apply_foreign_key(foreign_key, record.range.start, mode)?;
            }
            RecordKind::AutoIncrement => {
                reject_after_rows(saw_row, record.range.start, "auto-increment metadata")?;
                let auto_increment = decode_auto_increment_record(record.text, record.range.start)?;
                metadata.apply_auto_increment(auto_increment, record.range.clone(), mode)?;
            }
            RecordKind::Row => {
                if !saw_row {
                    row_start = record.range.start;
                    saw_row = true;
                }
                validate_row_record(record.text, record.range.start, |table| {
                    metadata.table(table)
                })?;
            }
            RecordKind::Unknown => {
                return Err(corrupt(record.range.start, "unknown record tag"));
            }
        }
    }

    let catalog = metadata.finish(row_start);
    if let Err(error) = integrity::validate_rows(blob, &catalog) {
        return Err(match error {
            integrity::ValidationError::Storage(error) => error,
            integrity::ValidationError::Constraint(violation) => {
                map_constraint_violation(violation, mode)
            }
        });
    }
    Ok(catalog)
}

fn reject_after_rows(saw_row: bool, offset: usize, record: &str) -> Result<()> {
    if saw_row {
        Err(corrupt(
            offset,
            format!("{record} appears after a row record"),
        ))
    } else {
        Ok(())
    }
}

fn map_constraint_violation(violation: integrity::Violation, mode: ValidationMode) -> Error {
    match mode {
        ValidationMode::Persisted => corrupt(violation.offset, violation.message),
        ValidationMode::Candidate => Error::Constraint(violation.message),
    }
}
