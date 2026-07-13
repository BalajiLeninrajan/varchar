//! Bounded physical edits over an authoritative database string.
//!
//! Edits must arrive in storage order. The builder copies untouched source
//! ranges and delegates record encoding back to the storage layer, so callers
//! never splice wire-format fragments themselves.

use std::ops::Range;

use super::encode::encode_auto_increment_record;
use super::{RowLayout, StorageState, TableSchema, encode_row, encode_schema};
use crate::{Error, Resource, Result, Value};

/// A bounded, ordered edit of one validated authoritative database string.
pub(crate) struct Candidate<'a> {
    state: &'a StorageState,
    cursor: usize,
    output: String,
    max_bytes: usize,
}

impl<'a> Candidate<'a> {
    pub(super) fn new(state: &'a StorageState, max_bytes: usize) -> Result<Self> {
        let source = state.as_str();
        check_size(source.len(), max_bytes)?;
        Ok(Self {
            state,
            cursor: 0,
            output: String::new(),
            max_bytes,
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
        let encoded = encode_auto_increment_record(schema, state.column, last)?;
        self.splice(state.record_range.clone(), &encoded)
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
            .map_err(|_| candidate_allocation_error())?;
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
            .map_err(|_| candidate_allocation_error())?;
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

fn candidate_allocation_error() -> Error {
    Error::Allocation {
        operation: "building a database candidate",
    }
}

fn invalid_range(offset: usize) -> Error {
    Error::CorruptStorage {
        offset,
        message: String::from("storage edit range is outside the database"),
    }
}

#[cfg(test)]
mod tests {
    use super::StorageState;

    #[test]
    fn new_candidate_does_not_allocate_before_its_first_edit() {
        let source = "V2;~S|t|id:I:!;~R|t|I1;";
        let state = StorageState::load(source.to_owned()).expect("source is valid");
        let candidate = state.candidate(source.len()).expect("source fits");

        assert_eq!(candidate.output.capacity(), 0);
    }

    #[test]
    fn failed_splice_leaves_the_candidate_reusable() {
        let source = "V2;~S|t|id:I:!;~R|t|I1;";
        let state = StorageState::load(source.to_owned()).expect("source is valid");
        let mut candidate = state.candidate(source.len()).expect("source fits");

        assert!(
            candidate
                .splice(3..source.len(), "replacement is too large")
                .is_err()
        );
        assert_eq!(
            candidate
                .finish()
                .expect("unchanged candidate fits")
                .as_str(),
            source
        );
    }
}
