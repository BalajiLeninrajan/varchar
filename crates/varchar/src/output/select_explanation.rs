use super::ResultColumn;

/// An immutable explanation of the source-row scan produced for a `SELECT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectExplanation {
    pattern: String,
    sources: Vec<String>,
    columns: Vec<ResultColumn>,
}

impl SelectExplanation {
    pub(crate) fn new(pattern: String, sources: Vec<String>, columns: Vec<ResultColumn>) -> Self {
        Self {
            pattern,
            sources,
            columns,
        }
    }

    /// The generated pattern used to select complete encoded rows.
    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
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
