use super::{RowSet, SelectExplanation};

/// The result of executing one statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A materialized `SELECT` result.
    Rows(RowSet),
    /// The result of an `INSERT`, `UPDATE`, or `DELETE`.
    Affected {
        /// Number of inserted, updated, or deleted rows.
        rows: usize,
    },
    /// The result of `CREATE TABLE`.
    Created {
        /// Normalized name of the created table.
        table: String,
    },
    /// A compiled `EXPLAIN REGEX` result.
    Explain(SelectExplanation),
}

impl Outcome {
    /// Whether this result came from a statement that mutates the database.
    #[must_use]
    pub const fn is_mutation(&self) -> bool {
        matches!(self, Self::Affected { .. } | Self::Created { .. })
    }
}
