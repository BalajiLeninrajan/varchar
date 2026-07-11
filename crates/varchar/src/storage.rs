//! Canonical storage and physical edits for the single-string database.

mod candidate;
mod catalog;
mod decode;
mod encode;
mod format;
mod integrity;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use crate::{Column, Error, Result};

pub(crate) use candidate::Candidate;
pub(crate) use catalog::{validate_and_catalog, validate_candidate};
pub(crate) use decode::{decode_row, row_table};
pub(crate) use encode::{encode_cell, encode_row, encode_schema};
pub(crate) use format::{
    cell_boundary_pattern, cell_pattern, encoded_text_literal_pattern, row_prefix_pattern,
    text_unit_pattern,
};

/// The canonical empty database.
pub(crate) const EMPTY_BLOB: &str = format::HEADER;

/// The derived schema index reconstructed from the authoritative string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Catalog {
    tables: BTreeMap<String, TableSchema>,
    auto_increments: BTreeMap<String, AutoIncrementState>,
    /// Byte offset at which another schema record can be inserted.
    row_start: usize,
}

impl Catalog {
    pub(crate) fn empty() -> Self {
        Self {
            tables: BTreeMap::new(),
            auto_increments: BTreeMap::new(),
            row_start: EMPTY_BLOB.len(),
        }
    }

    pub(crate) fn table(&self, name: &str) -> Option<&TableSchema> {
        self.tables.get(name)
    }

    pub(crate) fn auto_increment(&self, table: &str) -> Option<AutoIncrement> {
        self.auto_increments.get(table).map(|state| AutoIncrement {
            column: state.column,
            last: state.last,
        })
    }

    fn auto_increment_state(&self, table: &str) -> Option<&AutoIncrementState> {
        self.auto_increments.get(table)
    }
}

/// The logical portion of a table's persisted auto-increment state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AutoIncrement {
    pub(crate) column: usize,
    pub(crate) last: i64,
}

/// Storage-owned auto-increment state, including its physical edit range.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AutoIncrementState {
    column: usize,
    last: i64,
    record_range: Range<usize>,
}

/// The physical shape required to encode, decode, or scan one table's rows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowLayout<'a> {
    pub(crate) table: &'a str,
    pub(crate) columns: &'a [Column],
}

/// A table definition reconstructed from its schema and key metadata records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TableSchema {
    pub(crate) name: String,
    pub(crate) columns: Vec<Column>,
    pub(crate) primary_key: Option<usize>,
    pub(crate) foreign_keys: Vec<ForeignKey>,
}

impl TableSchema {
    pub(crate) fn row_layout(&self) -> RowLayout<'_> {
        RowLayout {
            table: &self.name,
            columns: &self.columns,
        }
    }
}

/// A single-column foreign key reconstructed from schema metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ForeignKey {
    /// Index of the referencing column in the local table.
    pub(crate) column: usize,
    pub(crate) referenced_table: String,
    pub(crate) referenced_column: String,
}

pub(crate) fn validate_schema_for_write(schema: &TableSchema) -> Result<()> {
    validate_row_layout(schema.row_layout())?;

    if let Some(primary_key) = schema.primary_key {
        let Some(column) = schema.columns.get(primary_key) else {
            return Err(Error::Schema(format!(
                "primary-key index {primary_key} is outside table {:?}",
                schema.name
            )));
        };
        if column.nullable {
            return Err(Error::Schema(format!(
                "primary-key column {:?}.{:?} must be NOT NULL",
                schema.name, column.name
            )));
        }
    }

    let mut foreign_key_columns = BTreeSet::new();
    for foreign_key in &schema.foreign_keys {
        if schema.columns.get(foreign_key.column).is_none() {
            return Err(Error::Schema(format!(
                "foreign-key index {} is outside table {:?}",
                foreign_key.column, schema.name
            )));
        }
        if !foreign_key_columns.insert(foreign_key.column) {
            return Err(Error::Schema(format!(
                "column {:?}.{:?} has multiple foreign keys",
                schema.name, schema.columns[foreign_key.column].name
            )));
        }
        if !format::is_valid_identifier(&foreign_key.referenced_table)
            || !format::is_valid_identifier(&foreign_key.referenced_column)
        {
            return Err(Error::Schema(format!(
                "invalid foreign-key target {:?}.{:?}",
                foreign_key.referenced_table, foreign_key.referenced_column
            )));
        }
    }
    Ok(())
}

pub(crate) fn validate_row_layout(layout: RowLayout<'_>) -> Result<()> {
    if !format::is_valid_identifier(layout.table) {
        return Err(Error::Schema(format!(
            "invalid or noncanonical table name {:?}",
            layout.table
        )));
    }
    if layout.columns.is_empty() {
        return Err(Error::Schema(String::from(
            "table must contain at least one column",
        )));
    }
    let mut names = BTreeSet::new();
    for column in layout.columns {
        if !format::is_valid_identifier(&column.name) {
            return Err(Error::Schema(format!(
                "invalid or noncanonical column name {:?}",
                column.name
            )));
        }
        if !names.insert(column.name.as_str()) {
            return Err(Error::Schema(format!(
                "duplicate column name {:?}",
                column.name
            )));
        }
    }
    Ok(())
}
