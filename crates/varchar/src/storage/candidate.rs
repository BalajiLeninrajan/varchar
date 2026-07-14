//! Bounded physical edits over an authoritative database string.
//!
//! Edits must arrive in storage order. The builder copies untouched source
//! ranges and delegates record encoding back to the storage layer, so callers
//! never splice wire-format fragments themselves.

use std::ops::Range;

use super::encode::encode_auto_increment_record;
use super::{RowLayout, StorageState, TableSchema, encode_row, encode_schema};
use crate::{Error, Resource, Result, Value};

struct DeferredAutoIncrement<'a> {
    table: &'a str,
    column: usize,
    last: i64,
    record_range: Range<usize>,
}

/// A bounded, ordered edit of one validated authoritative database string.
pub(crate) struct Candidate<'a> {
    state: &'a StorageState,
    cursor: usize,
    output: String,
    max_bytes: usize,
    deferred_auto_increment: Option<DeferredAutoIncrement<'a>>,
}

impl<'a> Candidate<'a> {
    pub(super) fn new(state: &'a StorageState, max_bytes: usize) -> Result<Self> {
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
            deferred_auto_increment: None,
        })
    }

    pub(crate) fn insert_schema_with_auto_increment(
        &mut self,
        schema: &TableSchema,
        auto_increment: Option<usize>,
    ) -> Result<()> {
        let encoded = encode_schema(schema)?;
        let encoded = if let Some(column) = auto_increment {
            encoded + &encode_auto_increment_record(schema, column, 0)?
        } else {
            encoded
        };
        let row_start = self.state.catalog().row_start;
        self.splice(row_start..row_start, &encoded)
    }

    pub(crate) fn advance_auto_increment(&mut self, table: &str, last: i64) -> Result<()> {
        let edit = self.auto_increment_edit(table, last)?;
        let encoded = self.encode_auto_increment_edit(&edit)?;
        self.splice(edit.record_range, &encoded)
    }

    /// Defer the sequence edit until a row is actually rewritten.
    ///
    /// `UPDATE` resolves assignments before it knows whether any row matches.
    /// Keeping only this logical edit avoids allocating, size-checking, or
    /// validating a larger metadata record for a zero-match statement.
    pub(crate) fn defer_auto_increment(&mut self, table: &str, last: i64) -> Result<()> {
        self.deferred_auto_increment = Some(self.auto_increment_edit(table, last)?);
        Ok(())
    }

    fn auto_increment_edit(&self, table: &str, last: i64) -> Result<DeferredAutoIncrement<'a>> {
        let catalog = self.state.catalog();
        let state = catalog.auto_increment_state(table).ok_or_else(|| {
            Error::schema(format!("table {table:?} has no auto-increment column"))
        })?;
        if last < state.last {
            return Err(Error::schema(format!(
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
        encode_auto_increment_record(schema, edit.column, edit.last)
    }

    fn apply_deferred_auto_increment(&mut self) -> Result<()> {
        let Some(edit) = self.deferred_auto_increment.take() else {
            return Ok(());
        };
        let encoded = self.encode_auto_increment_edit(&edit)?;
        self.splice(edit.record_range, &encoded)
    }

    pub(crate) fn append_row(&mut self, layout: RowLayout<'_>, values: &[Value]) -> Result<()> {
        let encoded = encode_row(values, layout)?;
        let source_len = self.state.as_str().len();
        self.splice(source_len..source_len, &encoded)
    }

    pub(crate) fn rewrite_row(
        &mut self,
        range: Range<usize>,
        layout: RowLayout<'_>,
        replacement: Option<&[Value]>,
    ) -> Result<()> {
        let encoded = replacement
            .map(|values| encode_row(values, layout))
            .transpose()?;
        self.apply_deferred_auto_increment()?;
        self.splice(range, encoded.as_deref().unwrap_or_default())
    }

    pub(crate) fn source(&self) -> &'a str {
        self.state.as_str()
    }

    pub(crate) fn finish(mut self) -> Result<StorageState> {
        self.push_source(self.cursor..self.state.as_str().len())?;
        StorageState::from_candidate(self.output)
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
    Error::resource_limit(Resource::DatabaseBytes, limit)
}

const fn allocation_error(operation: &'static str) -> Error {
    Error::allocation(operation)
}

fn invalid_range(offset: usize) -> Error {
    Error::corrupt_storage(offset, "storage edit range is outside the database")
}

#[cfg(test)]
mod tests;
