//! Derived schema and auto-increment indexes reconstructed from storage.

mod map;

use std::ops::Range;

pub(super) use map::CatalogMap;

use super::decode::{self, RowRecordRef};
use super::{EMPTY_BLOB, TableSchema, ValidatedTableSchema};

/// The derived schema index reconstructed from the authoritative string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Catalog {
    pub(super) tables: CatalogMap<TableSchema>,
    pub(super) auto_increments: CatalogMap<AutoIncrementState>,
    /// Byte offset at which another schema record can be inserted.
    pub(super) row_start: usize,
}

impl Catalog {
    pub(crate) fn empty() -> Self {
        Self {
            tables: CatalogMap::new(),
            auto_increments: CatalogMap::new(),
            row_start: EMPTY_BLOB.len(),
        }
    }

    pub(crate) fn table(&self, name: &str) -> Option<&TableSchema> {
        self.tables.get(name)
    }

    pub(crate) fn table_with_order(&self, name: &str) -> Option<(usize, &TableSchema)> {
        self.tables.get_with_order(name)
    }

    pub(crate) fn validated_table(&self, name: &str) -> Option<ValidatedTableSchema<'_>> {
        self.tables
            .get(name)
            .map(ValidatedTableSchema::from_catalog)
    }

    pub(super) fn schemas(&self) -> impl Iterator<Item = &TableSchema> {
        self.tables.values()
    }

    pub(crate) fn tables(&self) -> impl Iterator<Item = (&str, &TableSchema)> {
        self.tables.iter()
    }

    pub(crate) fn table_count(&self) -> usize {
        self.tables.len()
    }

    pub(crate) fn row_records<'a>(
        &self,
        blob: &'a str,
    ) -> impl Iterator<Item = crate::Result<RowRecordRef<'a>>> {
        decode::row_records(blob, self.row_start)
    }

    pub(crate) fn auto_increment(&self, table: &str) -> Option<AutoIncrement> {
        self.auto_increments.get(table).map(|state| AutoIncrement {
            column: state.column,
            last: state.last,
        })
    }

    pub(super) fn auto_increment_state(&self, table: &str) -> Option<&AutoIncrementState> {
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
pub(super) struct AutoIncrementState {
    pub(super) column: usize,
    pub(super) last: i64,
    pub(super) record_range: Range<usize>,
}
