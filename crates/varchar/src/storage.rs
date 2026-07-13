//! Canonical storage for the single-string database.

mod catalog;
mod decode;
mod encode;
mod format;

use std::collections::{BTreeMap, BTreeSet};

use crate::{Column, Error, Result};

pub(crate) use catalog::validate_and_catalog;
pub(crate) use decode::decode_row;
pub(crate) use encode::{encode_cell, encode_row, encode_schema};

/// The canonical empty database.
pub(crate) const EMPTY_BLOB: &str = format::HEADER;

/// The disposable schema index reconstructed from the authoritative string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Catalog {
    pub(crate) tables: BTreeMap<String, TableSchema>,
    /// Byte offset at which another schema record can be inserted.
    pub(crate) row_start: usize,
}

impl Catalog {
    pub(crate) fn table(&self, name: &str) -> Option<&TableSchema> {
        self.tables.get(name)
    }
}

/// A table definition reconstructed from a schema record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TableSchema {
    pub(crate) name: String,
    pub(crate) columns: Vec<Column>,
}

pub(crate) fn validate_schema_for_write(schema: &TableSchema) -> Result<()> {
    if !format::is_valid_identifier(&schema.name) {
        return Err(Error::Schema(format!(
            "invalid or noncanonical table name {:?}",
            schema.name
        )));
    }
    if schema.columns.is_empty() {
        return Err(Error::Schema(String::from(
            "table must contain at least one column",
        )));
    }
    let mut names = BTreeSet::new();
    for column in &schema.columns {
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
