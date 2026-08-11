//! Compiled query plans for reads, mutations, and public explanations.

use fancy_regex::Regex;

use crate::expression::Program;
use crate::output::SelectExplanation;
use crate::resolve::{ColumnLocation, ResolvedJoin, ResolvedOrderTerm};
use crate::storage::TableSchema;
use crate::{Result, SchemaColumn};

/// An owned mutation scan that remains valid while a candidate is assembled.
pub(crate) struct ScanPlan<'statement> {
    pub(super) regex: Regex,
    pub(super) table: String,
    pub(super) schema: Vec<SchemaColumn>,
    pub(super) local_residual: Option<Program<'statement>>,
}

/// A read-only plan borrowing the catalog schemas used by one `SELECT`.
pub(crate) struct SelectPlan<'catalog, 'statement> {
    pub(super) pattern: String,
    pub(super) regex: Regex,
    pub(super) sources: Vec<&'catalog TableSchema>,
    pub(super) projection: Vec<ColumnLocation>,
    pub(super) joins: Vec<ResolvedJoin>,
    pub(super) local_residuals: Vec<Option<Program<'statement>>>,
    pub(super) cross_source_residual: Option<Program<'statement>>,
    pub(super) order_by: Vec<ResolvedOrderTerm>,
    pub(super) limit: Option<u64>,
    pub(super) offset: Option<u64>,
}

impl SelectPlan<'_, '_> {
    pub(crate) fn into_explanation(
        self,
        max_query_output_bytes: usize,
    ) -> Result<SelectExplanation> {
        super::execute::explain(self, max_query_output_bytes)
    }
}
