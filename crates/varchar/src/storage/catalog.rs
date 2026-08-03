//! Derived schema and auto-increment indexes reconstructed from storage.

use std::collections::BTreeMap;
use std::ops::Range;

use super::{EMPTY_BLOB, TableSchema};

/// The derived schema index reconstructed from the authoritative string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Catalog {
    pub(super) tables: BTreeMap<String, TableSchema>,
    pub(super) auto_increments: BTreeMap<String, AutoIncrementState>,
    /// Byte offset at which another schema record can be inserted.
    pub(super) row_start: usize,
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

    pub(super) fn schemas(&self) -> impl Iterator<Item = &TableSchema> {
        self.tables.values()
    }

    pub(super) fn tables(&self) -> impl Iterator<Item = (&str, &TableSchema)> {
        self.tables
            .iter()
            .map(|(name, schema)| (name.as_str(), schema))
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
