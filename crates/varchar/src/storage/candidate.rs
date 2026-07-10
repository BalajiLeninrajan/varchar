//! Bounded physical edits over an authoritative database string.
//!
//! Edits must arrive in storage order. The builder copies untouched source
//! ranges and delegates record encoding back to the storage layer, so callers
//! never splice wire-format fragments themselves.

use std::ops::Range;

use super::{Catalog, RowLayout, TableSchema, encode_row, encode_schema};
use crate::{Error, Result, Value};

/// A bounded, ordered edit of one validated authoritative database string.
pub(crate) struct Candidate<'a> {
    source: &'a str,
    cursor: usize,
    output: String,
    max_bytes: usize,
}

impl<'a> Candidate<'a> {
    pub(crate) fn new(source: &'a str, max_bytes: usize) -> Result<Self> {
        check_size(source.len(), max_bytes)?;
        let mut output = String::new();
        output
            .try_reserve(source.len())
            .map_err(|_| limit_error(max_bytes))?;
        Ok(Self {
            source,
            cursor: 0,
            output,
            max_bytes,
        })
    }

    pub(crate) fn insert_schema(&mut self, catalog: &Catalog, schema: &TableSchema) -> Result<()> {
        let encoded = encode_schema(schema)?;
        self.splice(catalog.row_start..catalog.row_start, &encoded)
    }

    pub(crate) fn append_row(&mut self, layout: RowLayout<'_>, values: &[Value]) -> Result<()> {
        let encoded = encode_row(values, layout)?;
        self.splice(self.source.len()..self.source.len(), &encoded)
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

    pub(crate) fn finish(mut self) -> Result<String> {
        self.push_source(self.cursor..self.source.len())?;
        Ok(self.output)
    }

    fn splice(&mut self, range: Range<usize>, replacement: &str) -> Result<()> {
        if range.start < self.cursor || range.start > range.end {
            return Err(invalid_range(range.start));
        }
        self.source
            .get(range.clone())
            .ok_or_else(|| invalid_range(range.start))?;
        let gap = self
            .source
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
            .map_err(|_| limit_error(self.max_bytes))?;
        self.output.push_str(gap);
        self.output.push_str(replacement);
        self.cursor = range.end;
        Ok(())
    }

    fn push_source(&mut self, range: Range<usize>) -> Result<()> {
        let fragment = self
            .source
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
            .map_err(|_| limit_error(self.max_bytes))?;
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
        resource: "database bytes",
        limit,
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
    use super::Candidate;

    #[test]
    fn failed_splice_leaves_the_candidate_reusable() {
        let source = "V2;~R|t|I1;";
        let mut candidate = Candidate::new(source, source.len()).expect("source fits");

        assert!(
            candidate
                .splice(3..source.len(), "replacement is too large")
                .is_err()
        );
        assert_eq!(
            candidate.finish().expect("unchanged candidate fits"),
            source
        );
    }
}
