use crate::Column;

/// The exact regular expression and projection produced for a `SELECT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegexPlan {
    pub(crate) pattern: String,
    pub(crate) table: String,
    pub(crate) schema: Vec<Column>,
    pub(crate) projection: Vec<usize>,
}

impl RegexPlan {
    /// The generated pattern used to select complete encoded rows.
    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// The selected table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    /// Projected columns, in query order and including duplicates.
    #[must_use]
    pub fn columns(&self) -> Vec<Column> {
        self.projection
            .iter()
            .map(|&index| self.schema[index].clone())
            .collect()
    }
}
