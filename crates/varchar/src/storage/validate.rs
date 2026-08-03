//! Whole-blob validation and reconstruction of the derived schema catalog.

mod metadata;

use super::budget::WorkingBudget;
use super::decode::{
    decode_auto_increment_record, decode_check_record, decode_default_record,
    decode_foreign_key_record, decode_primary_key_record, decode_schema_record,
    decode_unique_record, validate_row_record,
};
use super::format::{FormatVersion, RecordKind, corrupt, decode_header, records};
use super::{Catalog, integrity};
use crate::{Error, Result};

use metadata::MetadataValidator;

#[derive(Clone, Copy)]
pub(super) enum ValidationMode {
    Persisted,
    Candidate,
}

/// Validate an authoritative blob and reconstruct its derived schema catalog.
#[cfg(test)]
pub(crate) fn validate_and_catalog(
    blob: &str,
    max_storage_working_bytes: usize,
) -> Result<(FormatVersion, Catalog)> {
    validate_and_catalog_with_limits(blob, max_storage_working_bytes, usize::MAX)
}

/// Validate an authoritative blob under the caller's CHECK limits.
pub(crate) fn validate_and_catalog_with_limits(
    blob: &str,
    max_storage_working_bytes: usize,
    max_predicates: usize,
) -> Result<(FormatVersion, Catalog)> {
    validate_with_mode(
        blob,
        ValidationMode::Persisted,
        max_storage_working_bytes,
        max_predicates,
    )
}

/// Validate a database assembled by a SQL mutation.
///
/// Structural encoding failures remain corruption errors. Invalid constraint
/// definitions are schema errors, while row-level violations are constraints.
pub(crate) fn validate_candidate(
    blob: &str,
    max_storage_working_bytes: usize,
    max_predicates: usize,
) -> Result<(FormatVersion, Catalog)> {
    validate_with_mode(
        blob,
        ValidationMode::Candidate,
        max_storage_working_bytes,
        max_predicates,
    )
}

fn validate_with_mode(
    blob: &str,
    mode: ValidationMode,
    max_storage_working_bytes: usize,
    max_predicates: usize,
) -> Result<(FormatVersion, Catalog)> {
    let version = decode_header(blob)?;
    let mut budget = WorkingBudget::new(max_storage_working_bytes);
    let mut metadata = MetadataValidator::new();
    let mut row_start = blob.len();
    let mut saw_row = false;

    for record in records(blob, version) {
        let record = record?;
        match record.kind {
            RecordKind::Schema => {
                reject_after_rows(saw_row, record.range.start, "schema record")?;
                let schema = decode_schema_record(record.text, record.range.start, &mut budget)?;
                metadata.insert_schema(schema, record.range.start, &mut budget)?;
            }
            RecordKind::PrimaryKey => {
                reject_after_rows(saw_row, record.range.start, "primary-key metadata")?;
                let primary_key = decode_primary_key_record(record.text, record.range.start)?;
                metadata.apply_primary_key(primary_key, record.range.start, mode)?;
            }
            RecordKind::ForeignKey => {
                reject_after_rows(saw_row, record.range.start, "foreign-key metadata")?;
                let foreign_key = decode_foreign_key_record(record.text, record.range.start)?;
                metadata.apply_foreign_key(foreign_key, record.range.start, mode, &mut budget)?;
            }
            RecordKind::AutoIncrement => {
                reject_after_rows(saw_row, record.range.start, "auto-increment metadata")?;
                let auto_increment = decode_auto_increment_record(record.text, record.range.start)?;
                metadata.apply_auto_increment(
                    auto_increment,
                    record.range.clone(),
                    mode,
                    &mut budget,
                )?;
            }
            RecordKind::Default => {
                reject_extension_in_v2(version, record.range.start)?;
                reject_after_rows(saw_row, record.range.start, "DEFAULT metadata")?;
                let default = decode_default_record(record.text, record.range.start)?;
                metadata.apply_default(default, record.range.start, mode, &mut budget)?;
            }
            RecordKind::Unique => {
                reject_extension_in_v2(version, record.range.start)?;
                reject_after_rows(saw_row, record.range.start, "UNIQUE metadata")?;
                let unique = decode_unique_record(record.text, record.range.start)?;
                metadata.apply_unique(unique, record.range.start, mode, &mut budget)?;
            }
            RecordKind::Check => {
                reject_extension_in_v2(version, record.range.start)?;
                reject_after_rows(saw_row, record.range.start, "CHECK metadata")?;
                let check = decode_check_record(record.text, record.range.start)?;
                metadata.apply_check(
                    check,
                    record.range.start,
                    mode,
                    max_predicates,
                    &mut budget,
                )?;
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
    if let Err(error) = integrity::validate_rows(blob, &catalog, &mut budget) {
        return Err(match error {
            integrity::ValidationError::Storage(error) => error,
            integrity::ValidationError::Constraint(violation) => {
                map_constraint_violation(violation, mode)
            }
        });
    }
    Ok((version, catalog))
}

fn reject_extension_in_v2(version: FormatVersion, offset: usize) -> Result<()> {
    if version.supports_extensions() {
        Ok(())
    } else {
        Err(corrupt(offset, "V3 metadata is invalid under a V2 header"))
    }
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
