use super::ResultColumn;
use crate::value::Value;

/// An immutable, materialized snapshot returned by a `SELECT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowSet {
    columns: Vec<ResultColumn>,
    rows: Vec<Vec<Value>>,
}

impl RowSet {
    pub(crate) fn new(columns: Vec<ResultColumn>, rows: Vec<Vec<Value>>) -> Self {
        debug_assert!(
            rows.iter().all(|row| row.len() == columns.len()),
            "every result row must match the projected column width"
        );
        Self { columns, rows }
    }

    /// Projected columns, in query order and including duplicates.
    #[must_use]
    pub fn columns(&self) -> &[ResultColumn] {
        &self.columns
    }

    /// Materialized rows in deterministic query order.
    #[must_use]
    pub fn rows(&self) -> &[Vec<Value>] {
        &self.rows
    }

    /// Consume this result and return its materialized rows.
    #[must_use]
    pub fn into_rows(self) -> Vec<Vec<Value>> {
        self.rows
    }

    /// Consume this result and return its column metadata and rows.
    #[must_use]
    pub fn into_parts(self) -> (Vec<ResultColumn>, Vec<Vec<Value>>) {
        (self.columns, self.rows)
    }
}
