//! Whole-blob validation and reconstruction of the derived schema catalog.

use std::collections::BTreeMap;
use std::ops::Range;

use super::decode::{
    AutoIncrementMetadata, ForeignKeyMetadata, PrimaryKeyMetadata, decode_auto_increment_record,
    decode_foreign_key_record, decode_primary_key_record, decode_schema_record,
    validate_row_record,
};
use super::format::{HEADER, RecordKind, corrupt, records};
use super::{AutoIncrementState, Catalog, ForeignKey, TableSchema, integrity};
use crate::{DataType, Error, Result};

#[derive(Clone, Copy)]
enum ValidationMode {
    Persisted,
    Candidate,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MetadataPhase {
    None,
    PrimaryOrForeignKey,
    ForeignKeys,
    Complete,
}

struct MetadataState {
    table: String,
    phase: MetadataPhase,
    next_foreign_key_column: usize,
}

impl MetadataState {
    fn none() -> Self {
        Self {
            table: String::new(),
            phase: MetadataPhase::None,
            next_foreign_key_column: 0,
        }
    }

    fn begin_table(table: &str) -> Self {
        Self {
            table: table.to_owned(),
            phase: MetadataPhase::PrimaryOrForeignKey,
            next_foreign_key_column: 0,
        }
    }
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

    let mut tables = BTreeMap::<String, TableSchema>::new();
    let mut auto_increments = BTreeMap::<String, AutoIncrementState>::new();
    let mut metadata = MetadataState::none();
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
                metadata = MetadataState::begin_table(&schema.name);
                tables.insert(schema.name.clone(), schema);
            }
            RecordKind::PrimaryKey => {
                if saw_row {
                    return Err(corrupt(
                        record.range.start,
                        "primary-key metadata appears after a row record",
                    ));
                }
                let primary_key = decode_primary_key_record(record.text, record.range.start)?;
                apply_primary_key(&mut tables, &mut metadata, primary_key, record.range.start)
                    .map_err(|violation| map_schema_violation(violation, mode))?;
            }
            RecordKind::ForeignKey => {
                if saw_row {
                    return Err(corrupt(
                        record.range.start,
                        "foreign-key metadata appears after a row record",
                    ));
                }
                let foreign_key = decode_foreign_key_record(record.text, record.range.start)?;
                apply_foreign_key(&mut tables, &mut metadata, foreign_key, record.range.start)
                    .map_err(|violation| map_schema_violation(violation, mode))?;
            }
            RecordKind::AutoIncrement => {
                if saw_row {
                    return Err(corrupt(
                        record.range.start,
                        "auto-increment metadata appears after a row record",
                    ));
                }
                let auto_increment = decode_auto_increment_record(record.text, record.range.start)?;
                apply_auto_increment(
                    &tables,
                    &mut auto_increments,
                    &mut metadata,
                    auto_increment,
                    record.range.clone(),
                )
                .map_err(|violation| map_schema_violation(violation, mode))?;
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

    let catalog = Catalog {
        tables,
        auto_increments,
        row_start,
    };
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

fn apply_primary_key(
    tables: &mut BTreeMap<String, TableSchema>,
    state: &mut MetadataState,
    metadata: PrimaryKeyMetadata<'_>,
    offset: usize,
) -> std::result::Result<(), Violation> {
    if state.phase != MetadataPhase::PrimaryOrForeignKey || metadata.table != state.table {
        return Err(Violation::new(
            offset,
            "primary-key metadata must immediately follow its table schema",
        ));
    }
    let schema = tables
        .get_mut(&state.table)
        .expect("metadata state always names the most recent schema");
    let Some(column) = schema
        .columns
        .iter()
        .position(|column| column.name == metadata.column)
    else {
        return Err(Violation::new(
            offset,
            format!(
                "primary key for table {:?} references unknown column {:?}",
                metadata.table, metadata.column
            ),
        ));
    };
    if schema.columns[column].nullable {
        return Err(Violation::new(
            offset,
            format!(
                "primary-key column {:?}.{:?} must be NOT NULL",
                metadata.table, metadata.column
            ),
        ));
    }
    schema.primary_key = Some(column);
    state.phase = MetadataPhase::ForeignKeys;
    Ok(())
}

fn apply_foreign_key(
    tables: &mut BTreeMap<String, TableSchema>,
    state: &mut MetadataState,
    metadata: ForeignKeyMetadata<'_>,
    offset: usize,
) -> std::result::Result<(), Violation> {
    if !matches!(
        state.phase,
        MetadataPhase::PrimaryOrForeignKey | MetadataPhase::ForeignKeys
    ) || metadata.table != state.table
    {
        return Err(Violation::new(
            offset,
            "foreign-key metadata must immediately follow its table schema or another key",
        ));
    }

    let (column, data_type) = {
        let schema = tables
            .get(&state.table)
            .expect("metadata state always names the most recent schema");
        let remaining_columns = &schema.columns[state.next_foreign_key_column..];
        let Some(relative_column) = remaining_columns
            .iter()
            .position(|column| column.name == metadata.column)
        else {
            let message = if schema.columns[..state.next_foreign_key_column]
                .iter()
                .any(|column| column.name == metadata.column)
            {
                String::from("foreign-key metadata is not in increasing local-column order")
            } else {
                format!(
                    "foreign key for table {:?} references unknown local column {:?}",
                    metadata.table, metadata.column
                )
            };
            return Err(Violation::new(offset, message));
        };
        let column = state.next_foreign_key_column + relative_column;
        (column, schema.columns[column].data_type)
    };

    let Some(referenced_schema) = tables.get(metadata.referenced_table) else {
        return Err(Violation::new(
            offset,
            format!(
                "foreign key references unknown or later table {:?}",
                metadata.referenced_table
            ),
        ));
    };
    let Some(referenced_column) = referenced_schema.primary_key else {
        return Err(Violation::new(
            offset,
            format!(
                "foreign key target {:?}.{:?} is not its table's primary key",
                metadata.referenced_table, metadata.referenced_column
            ),
        ));
    };
    if referenced_schema.columns[referenced_column].name != metadata.referenced_column {
        return Err(Violation::new(
            offset,
            format!(
                "foreign key target {:?}.{:?} is not its table's primary key",
                metadata.referenced_table, metadata.referenced_column
            ),
        ));
    }
    if data_type != referenced_schema.columns[referenced_column].data_type {
        return Err(Violation::new(
            offset,
            format!(
                "foreign-key columns {:?}.{:?} and {:?}.{:?} have different types",
                metadata.table,
                metadata.column,
                metadata.referenced_table,
                metadata.referenced_column
            ),
        ));
    }

    tables
        .get_mut(&state.table)
        .expect("metadata state always names the most recent schema")
        .foreign_keys
        .push(ForeignKey {
            column,
            referenced_table: metadata.referenced_table.to_owned(),
            referenced_column: metadata.referenced_column.to_owned(),
        });
    state.phase = MetadataPhase::ForeignKeys;
    state.next_foreign_key_column = column + 1;
    Ok(())
}

fn apply_auto_increment(
    tables: &BTreeMap<String, TableSchema>,
    auto_increments: &mut BTreeMap<String, AutoIncrementState>,
    state: &mut MetadataState,
    metadata: AutoIncrementMetadata<'_>,
    record_range: Range<usize>,
) -> std::result::Result<(), Violation> {
    let offset = record_range.start;
    if state.phase != MetadataPhase::ForeignKeys || metadata.table != state.table {
        return Err(Violation::new(
            offset,
            "auto-increment metadata must follow its table's primary and foreign keys",
        ));
    }
    if metadata.last < 0 {
        return Err(Violation::new(
            offset,
            "auto-increment high-water mark must be nonnegative",
        ));
    }

    let schema = tables
        .get(&state.table)
        .expect("metadata state always names the most recent schema");
    let Some(primary_key) = schema.primary_key else {
        return Err(Violation::new(
            offset,
            "auto-increment column must be the table's INTEGER primary key",
        ));
    };
    let column = &schema.columns[primary_key];
    if column.name != metadata.column || column.data_type != DataType::Integer {
        return Err(Violation::new(
            offset,
            format!(
                "auto-increment column {:?}.{:?} must be its INTEGER primary key",
                metadata.table, metadata.column
            ),
        ));
    }

    auto_increments.insert(
        metadata.table.to_owned(),
        AutoIncrementState {
            column: primary_key,
            last: metadata.last,
            record_range,
        },
    );
    state.phase = MetadataPhase::Complete;
    Ok(())
}

struct Violation {
    offset: usize,
    message: String,
}

impl Violation {
    fn new(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset,
            message: message.into(),
        }
    }
}

fn map_schema_violation(violation: Violation, mode: ValidationMode) -> Error {
    match mode {
        ValidationMode::Persisted => corrupt(violation.offset, violation.message),
        ValidationMode::Candidate => Error::schema(violation.message),
    }
}

fn map_constraint_violation(violation: integrity::Violation, mode: ValidationMode) -> Error {
    match mode {
        ValidationMode::Persisted => corrupt(violation.offset, violation.message),
        ValidationMode::Candidate => Error::constraint(violation.message),
    }
}
