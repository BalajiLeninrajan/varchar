//! Table schemas and the physical row layouts they define.

use super::format;
use crate::{Error, Result, SchemaColumn};

/// The physical shape required to encode, decode, or scan one table's rows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowLayout<'a> {
    pub(crate) table: &'a str,
    pub(crate) columns: &'a [SchemaColumn],
}

/// Proof that a physical row layout passed canonical schema validation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ValidatedRowLayout<'a> {
    layout: RowLayout<'a>,
}

impl<'a> ValidatedRowLayout<'a> {
    #[cfg(test)]
    pub(crate) const fn column_count(self) -> usize {
        self.layout.columns.len()
    }

    pub(super) const fn layout(self) -> RowLayout<'a> {
        self.layout
    }
}

/// A table definition reconstructed from its schema and key metadata records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TableSchema {
    pub(crate) name: String,
    pub(crate) columns: Vec<SchemaColumn>,
    pub(crate) primary_key: Option<usize>,
    /// Non-primary single-column UNIQUE constraints in column order.
    pub(crate) unique_columns: Vec<usize>,
    /// Increasing by local column; each local column appears at most once.
    pub(crate) foreign_keys: Vec<ForeignKey>,
    /// Resolved CHECK expressions in declaration order.
    pub(crate) checks: Vec<crate::expression::CheckProgram>,
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

pub(crate) fn validate_row_layout<'layout>(
    layout: RowLayout<'layout>,
) -> Result<ValidatedRowLayout<'layout>> {
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
    // Replacement encoding invokes this path under the storage-working budget,
    // so inspect the borrowed prefix instead of allocating a temporary set.
    for (position, column) in layout.columns.iter().enumerate() {
        if !format::is_valid_identifier(&column.name) {
            return Err(Error::Schema(format!(
                "invalid or noncanonical column name {:?}",
                column.name
            )));
        }
        if layout.columns[..position]
            .iter()
            .any(|seen| seen.name == column.name)
        {
            return Err(Error::Schema(format!(
                "duplicate column name {:?}",
                column.name
            )));
        }
    }
    Ok(ValidatedRowLayout { layout })
}
