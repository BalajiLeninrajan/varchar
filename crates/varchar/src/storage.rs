//! Canonical storage and physical edits for the single-string database.

mod candidate;
mod catalog;
mod decode;
mod encode;
mod format;

use std::collections::{BTreeMap, BTreeSet};

use crate::{Column, Error, Result};

pub(crate) use candidate::Candidate;
pub(crate) use decode::decode_row;
pub(crate) use encode::{encode_cell, encode_row, encode_schema};
pub(crate) use format::{
    cell_boundary_pattern, cell_pattern, encoded_text_literal_pattern, row_prefix_pattern,
    text_unit_pattern,
};

use catalog::validate_and_catalog;

/// The canonical empty database.
pub(crate) const EMPTY_BLOB: &str = format::HEADER;

/// One validated authoritative blob and the catalog derived from that exact blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StorageState {
    blob: String,
    catalog: Catalog,
}

impl StorageState {
    pub(crate) fn empty() -> Self {
        Self {
            blob: EMPTY_BLOB.to_owned(),
            catalog: Catalog::empty(),
        }
    }

    pub(crate) fn load(blob: String) -> Result<Self> {
        let catalog = validate_and_catalog(&blob)?;
        Ok(Self { blob, catalog })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.blob
    }

    pub(crate) fn into_string(self) -> String {
        self.blob
    }

    pub(crate) fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub(crate) fn candidate(&self, max_bytes: usize) -> Result<Candidate<'_>> {
        Candidate::new(self, max_bytes)
    }
}

/// The derived schema index reconstructed from the authoritative string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Catalog {
    tables: BTreeMap<String, TableSchema>,
    /// Byte offset at which another schema record can be inserted.
    row_start: usize,
}

impl Catalog {
    pub(crate) fn empty() -> Self {
        Self {
            tables: BTreeMap::new(),
            row_start: EMPTY_BLOB.len(),
        }
    }

    pub(crate) fn table(&self, name: &str) -> Option<&TableSchema> {
        self.tables.get(name)
    }
}

/// The physical shape required to encode, decode, or scan one table's rows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowLayout<'a> {
    pub(crate) table: &'a str,
    pub(crate) columns: &'a [Column],
}

/// A table definition reconstructed from a schema record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TableSchema {
    pub(crate) name: String,
    pub(crate) columns: Vec<Column>,
}

impl TableSchema {
    pub(crate) fn row_layout(&self) -> RowLayout<'_> {
        RowLayout {
            table: &self.name,
            columns: &self.columns,
        }
    }
}

pub(crate) fn validate_schema_for_write(schema: &TableSchema) -> Result<()> {
    validate_row_layout(schema.row_layout())
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

#[cfg(test)]
mod tests;
