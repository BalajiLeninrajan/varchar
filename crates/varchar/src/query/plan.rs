//! Compiled query plans for reads, mutations, and public explanations.

use fancy_regex::Regex;

use crate::output::SelectExplanation;
use crate::resolve::{ColumnLocation, ResolvedJoin};
use crate::storage::TableSchema;
use crate::{Result, SchemaColumn};

/// An owned mutation scan that remains valid while a candidate is assembled.
pub(crate) struct ScanPlan {
    pub(super) regex: Regex,
    pub(super) table: String,
    pub(super) schema: Vec<SchemaColumn>,
}

/// A read-only plan borrowing the catalog schemas used by one `SELECT`.
pub(crate) struct SelectPlan<'catalog> {
    pub(super) pattern: String,
    pub(super) regex: Regex,
    pub(super) sources: Vec<&'catalog TableSchema>,
    pub(super) projection: Vec<ColumnLocation>,
    pub(super) joins: Vec<ResolvedJoin>,
}

impl SelectPlan<'_> {
    pub(crate) fn into_explanation(
        self,
        max_query_output_bytes: usize,
    ) -> Result<SelectExplanation> {
        super::execute::explain(self, max_query_output_bytes)
    }
}
