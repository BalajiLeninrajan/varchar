use super::ResultColumn;

/// An immutable explanation of the source-row scan produced for a `SELECT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectExplanation {
    pattern: String,
    pattern_is_exact: bool,
    sources: Vec<String>,
    columns: Vec<ResultColumn>,
}

impl SelectExplanation {
    pub(crate) fn new(
        pattern: String,
        pattern_is_exact: bool,
        sources: Vec<String>,
        columns: Vec<ResultColumn>,
    ) -> Self {
        Self {
            pattern,
            pattern_is_exact,
            sources,
            columns,
        }
    }

    /// The generated prefilter pattern used to select complete encoded rows.
    ///
    /// Residual Boolean evaluation and join `ON` conditions may still discard
    /// matched rows; [`Self::pattern_is_exact`] reports whether they can.
    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Whether row filtering is fully expressed by [`Self::pattern`].
    ///
    /// `true` means every predicate was pushed into the scan pattern and no
    /// Rust-side filtering runs against decoded values, so the pattern selects
    /// exactly the encoded rows the query retains. `false` means the pattern is
    /// a prefilter that can match rows the query rejects; a caller applying the
    /// pattern on its own over-selects.
    ///
    /// A multi-source explanation is never exact. The pattern for a join is an
    /// alternation over the whole rows of every source, and `JOIN ... ON`
    /// conditions are evaluated in Rust during the nested loops rather than
    /// pushed into the pattern, so the pattern matches source rows that the
    /// join discards.
    ///
    /// It deliberately does not describe the result set as a whole. Clauses
    /// that never eliminate source rows — projection, and any row ordering or
    /// pagination the dialect supports — are not represented by the pattern
    /// either, and they do not make this flag `false`.
    #[must_use]
    pub const fn pattern_is_exact(&self) -> bool {
        self.pattern_is_exact
    }

    /// Source tables in `FROM`/`JOIN` order.
    #[must_use]
    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    /// Projected columns, in query order and including duplicates.
    #[must_use]
    pub fn columns(&self) -> &[ResultColumn] {
        &self.columns
    }
}
