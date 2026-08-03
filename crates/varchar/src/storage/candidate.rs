//! Bounded physical edits over an authoritative database string.
//!
//! Edits must arrive in storage order. The builder copies untouched source
//! ranges and accepts replacement records produced by the storage codec before
//! final validation of the complete candidate.

use std::ops::Range;

use super::encode::{
    encode_auto_increment_record_prevalidated, encode_table_metadata,
    encoded_auto_increment_record_len_prevalidated, measure_table_metadata,
};
use super::format::{FormatVersion, V2_HEADER, V3_HEADER};
use super::{
    ForeignKeyDeleteAction, ForeignKeyUpdateAction, RowLayout, StorageState, TableSchema,
    encode_row,
};
use crate::{Error, Resource, Result, Value};

struct DeferredAutoIncrement<'a> {
    table: &'a str,
    column: usize,
    last: i64,
    record_range: Range<usize>,
}

impl DeferredAutoIncrement<'_> {
    const fn empty() -> Self {
        Self {
            table: "",
            column: 0,
            last: 0,
            record_range: 0..0,
        }
    }

    fn is_empty(&self) -> bool {
        self.record_range.is_empty()
    }
}

/// A bounded, ordered edit of one validated authoritative database string.
pub(crate) struct Candidate<'a> {
    state: &'a StorageState,
    cursor: usize,
    output: String,
    max_bytes: usize,
    max_predicates: usize,
    check_like_work_limit: usize,
    format: FormatVersion,
    deferred_auto_increments: Vec<DeferredAutoIncrement<'a>>,
}

impl<'a> Candidate<'a> {
    pub(super) fn new(
        state: &'a StorageState,
        max_bytes: usize,
        max_predicates: usize,
        check_like_work_limit: usize,
    ) -> Result<Self> {
        let source = state.as_str();
        check_size(source.len(), max_bytes)?;
        let mut output = String::new();
        output
            .try_reserve(source.len())
            .map_err(|_| allocation_error("reserving a storage edit candidate"))?;
        Ok(Self {
            state,
            cursor: 0,
            output,
            max_bytes,
            max_predicates,
            check_like_work_limit,
            format: state.format(),
            deferred_auto_increments: Vec::new(),
        })
    }

    pub(crate) fn insert_schema_with_auto_increment(
        &mut self,
        schema: &TableSchema,
        auto_increment: Option<usize>,
    ) -> Result<()> {
        let requires_v3 = schema.columns.iter().any(|column| column.default.is_some())
            || !schema.unique_columns.is_empty()
            || !schema.checks.is_empty()
            || schema.foreign_keys.iter().any(|foreign_key| {
                foreign_key.on_delete != ForeignKeyDeleteAction::Restrict
                    || foreign_key.on_update != ForeignKeyUpdateAction::Restrict
            });
        let auto_increment = auto_increment.map(|column| (column, 0));
        let measured = measure_table_metadata(schema, auto_increment)?;
        let upgrade_to_v3 = requires_v3 && self.format == FormatVersion::V2;
        if requires_v3 && !upgrade_to_v3 && !self.format.supports_extensions() {
            return Err(Error::Schema(String::from(
                "extended schema metadata requires storage format V3",
            )));
        }

        let replacement_header = if upgrade_to_v3 {
            V3_HEADER
        } else {
            self.format.header()
        };
        self.check_projected_table_insert_size(replacement_header, measured.encoded_len())?;
        let encoded = encode_table_metadata(schema, auto_increment, measured)?;

        if upgrade_to_v3 {
            self.splice(0..V2_HEADER.len(), V3_HEADER)?;
            self.format = FormatVersion::V3;
        }
        let row_start = self.state.catalog().row_start;
        self.splice(row_start..row_start, &encoded)
    }

    pub(crate) fn advance_auto_increment(&mut self, table: &str, last: i64) -> Result<()> {
        let edit = self.auto_increment_edit(table, last)?;
        let encoded = self.encode_auto_increment_edit(&edit)?;
        self.splice(edit.record_range, &encoded)
    }

    /// Defer sequence edits until a row is actually rewritten.
    ///
    /// `UPDATE` resolves assignments before it knows whether any row matches.
    /// Keeping only these logical edits avoids allocating or applying replacement
    /// metadata records for a zero-match statement.
    pub(crate) fn defer_auto_increment(&mut self, table: &str, last: i64) -> Result<()> {
        let (table_order, _) = self
            .state
            .catalog()
            .table_with_order(table)
            .ok_or_else(|| Error::Schema(format!("unknown table {table:?}")))?;
        let edit = self.auto_increment_edit(table, last)?;
        if self.deferred_auto_increments.is_empty() {
            let table_count = self.state.catalog().table_count();
            self.deferred_auto_increments
                .try_reserve_exact(table_count)
                .map_err(|_| allocation_error("reserving deferred auto-increment edits"))?;
            self.deferred_auto_increments
                .resize_with(table_count, DeferredAutoIncrement::empty);
        }

        let slot = self
            .deferred_auto_increments
            .get_mut(table_order)
            .expect("catalog table order fits the deferred edit index");
        if slot.is_empty() {
            *slot = edit;
        } else {
            debug_assert_eq!(slot.table, edit.table);
            slot.last = slot.last.max(edit.last);
        }
        Ok(())
    }

    pub(crate) fn deferred_auto_increment_reservation_bytes(&self) -> Result<usize> {
        if self.deferred_auto_increments.is_empty() {
            self.state
                .catalog()
                .table_count()
                .checked_mul(std::mem::size_of::<DeferredAutoIncrement<'_>>())
                .ok_or(Error::Capacity {
                    operation: "measuring deferred auto-increment edits",
                })
        } else {
            Ok(0)
        }
    }

    pub(crate) fn deferred_auto_increment_working_bytes(&self) -> usize {
        if self.deferred_auto_increments.is_empty() {
            0
        } else {
            self.state
                .catalog()
                .table_count()
                .saturating_mul(std::mem::size_of::<DeferredAutoIncrement<'_>>())
        }
    }

    /// Aggregate source and replacement bytes across all deferred sequence edits.
    pub(crate) fn deferred_auto_increment_lengths(&self) -> Result<Option<(usize, usize)>> {
        self.deferred_auto_increment_length_summary()
            .map(|summary| summary.map(|(original, replacement, _)| (original, replacement)))
    }

    /// Largest single replacement allocation needed while applying deferred edits.
    pub(crate) fn deferred_auto_increment_max_replacement_bytes(&self) -> Result<Option<usize>> {
        self.deferred_auto_increment_length_summary()
            .map(|summary| summary.map(|(_, _, max_replacement)| max_replacement))
    }

    fn deferred_auto_increment_length_summary(&self) -> Result<Option<(usize, usize, usize)>> {
        if self.deferred_auto_increments.is_empty() {
            return Ok(None);
        }

        let mut original = 0_usize;
        let mut replacement = 0_usize;
        let mut max_replacement = 0_usize;
        for edit in self
            .deferred_auto_increments
            .iter()
            .filter(|edit| !edit.is_empty())
        {
            let schema = self
                .state
                .catalog()
                .table(edit.table)
                .expect("a deferred auto-increment edit names a catalog table");
            let edit_replacement =
                encoded_auto_increment_record_len_prevalidated(schema, edit.column, edit.last)?;
            original = original
                .checked_add(edit.record_range.len())
                .ok_or(Error::Capacity {
                    operation: "measuring deferred auto-increment source records",
                })?;
            replacement = replacement
                .checked_add(edit_replacement)
                .ok_or(Error::Capacity {
                    operation: "measuring deferred auto-increment replacement records",
                })?;
            max_replacement = max_replacement.max(edit_replacement);
        }
        Ok(Some((original, replacement, max_replacement)))
    }

    fn auto_increment_edit(&self, table: &str, last: i64) -> Result<DeferredAutoIncrement<'a>> {
        let catalog = self.state.catalog();
        let state = catalog.auto_increment_state(table).ok_or_else(|| {
            Error::Schema(format!("table {table:?} has no auto-increment column"))
        })?;
        if last < state.last {
            return Err(Error::Schema(format!(
                "auto-increment high-water mark for table {table:?} cannot decrease"
            )));
        }
        let schema = catalog
            .table(table)
            .expect("auto-increment state always names a catalog table");
        Ok(DeferredAutoIncrement {
            table: &schema.name,
            column: state.column,
            last,
            record_range: state.record_range.clone(),
        })
    }

    fn encode_auto_increment_edit(&self, edit: &DeferredAutoIncrement<'_>) -> Result<String> {
        let schema = self
            .state
            .catalog()
            .table(edit.table)
            .expect("a deferred auto-increment edit names a catalog table");
        encode_auto_increment_record_prevalidated(schema, edit.column, edit.last)
    }

    fn apply_deferred_auto_increment(&mut self) -> Result<()> {
        let edits = std::mem::take(&mut self.deferred_auto_increments);
        for edit in edits.into_iter().filter(|edit| !edit.is_empty()) {
            let encoded = self.encode_auto_increment_edit(&edit)?;
            self.splice(edit.record_range, &encoded)?;
        }
        Ok(())
    }

    pub(crate) fn append_row(&mut self, layout: RowLayout<'_>, values: &[Value]) -> Result<()> {
        let encoded = encode_row(values, layout)?;
        let source_len = self.state.as_str().len();
        self.splice(source_len..source_len, &encoded)
    }

    pub(crate) fn rewrite_encoded_row(
        &mut self,
        range: Range<usize>,
        replacement: Option<&str>,
    ) -> Result<()> {
        self.apply_deferred_auto_increment()?;
        self.splice(range, replacement.unwrap_or_default())
    }

    pub(crate) fn source(&self) -> &'a str {
        self.state.as_str()
    }

    pub(crate) fn catalog(&self) -> &super::Catalog {
        self.state.catalog()
    }

    fn discard_deferred_auto_increment(&mut self) {
        let _ = std::mem::take(&mut self.deferred_auto_increments);
    }

    pub(crate) fn finish(mut self) -> Result<StorageState> {
        self.discard_deferred_auto_increment();
        self.push_source(self.cursor..self.state.as_str().len())?;
        StorageState::from_candidate(
            self.output,
            self.max_bytes,
            self.max_predicates,
            self.check_like_work_limit,
        )
    }

    fn check_projected_table_insert_size(
        &self,
        replacement_header: &str,
        metadata_len: usize,
    ) -> Result<()> {
        let without_header = self
            .state
            .as_str()
            .len()
            .checked_sub(self.format.header().len())
            .ok_or_else(|| limit_error(self.max_bytes))?;
        let with_header = without_header
            .checked_add(replacement_header.len())
            .ok_or_else(|| limit_error(self.max_bytes))?;
        let projected = with_header
            .checked_add(metadata_len)
            .ok_or_else(|| limit_error(self.max_bytes))?;
        check_size(projected, self.max_bytes)
    }

    fn splice(&mut self, range: Range<usize>, replacement: &str) -> Result<()> {
        if range.start < self.cursor || range.start > range.end {
            return Err(invalid_range(range.start));
        }
        self.state
            .as_str()
            .get(range.clone())
            .ok_or_else(|| invalid_range(range.start))?;
        let gap = self
            .state
            .as_str()
            .get(self.cursor..range.start)
            .ok_or_else(|| invalid_range(range.start))?;
        let additional = gap
            .len()
            .checked_add(replacement.len())
            .ok_or_else(|| limit_error(self.max_bytes))?;
        let new_len = self
            .output
            .len()
            .checked_add(additional)
            .ok_or_else(|| limit_error(self.max_bytes))?;
        check_size(new_len, self.max_bytes)?;
        self.output
            .try_reserve(additional)
            .map_err(|_| allocation_error("reserving a storage edit candidate"))?;
        self.output.push_str(gap);
        self.output.push_str(replacement);
        self.cursor = range.end;
        Ok(())
    }

    fn push_source(&mut self, range: Range<usize>) -> Result<()> {
        let fragment = self
            .state
            .as_str()
            .get(range.clone())
            .ok_or_else(|| invalid_range(range.start))?;
        self.push(fragment)
    }

    fn push(&mut self, fragment: &str) -> Result<()> {
        let new_len = self
            .output
            .len()
            .checked_add(fragment.len())
            .ok_or_else(|| limit_error(self.max_bytes))?;
        check_size(new_len, self.max_bytes)?;
        self.output
            .try_reserve(fragment.len())
            .map_err(|_| allocation_error("reserving a storage edit candidate"))?;
        self.output.push_str(fragment);
        Ok(())
    }
}

fn check_size(actual: usize, limit: usize) -> Result<()> {
    if actual > limit {
        Err(limit_error(limit))
    } else {
        Ok(())
    }
}

fn limit_error(limit: usize) -> Error {
    Error::ResourceLimit {
        resource: Resource::DatabaseBytes,
        limit,
    }
}

const fn allocation_error(operation: &'static str) -> Error {
    Error::Allocation { operation }
}

fn invalid_range(offset: usize) -> Error {
    Error::CorruptStorage {
        offset,
        message: String::from("storage edit range is outside the database"),
    }
}

#[cfg(test)]
mod tests;
