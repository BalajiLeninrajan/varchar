//! Compiled query plans for reads, mutations, and public explanations.

use fancy_regex::Regex;

use crate::Result;
use crate::expression::Program;
use crate::output::SelectExplanation;
use crate::resolve::{ColumnLocation, ResolvedJoin, ResolvedOrderTerm};
#[cfg(test)]
use crate::storage::ValidatedRowLayout;
use crate::storage::{OwnedValidatedRowLayout, RowLayout, TableSchema};

/// An owned mutation scan that remains valid while a candidate is assembled.
pub(crate) struct ScanPlan<'statement> {
    pub(super) regex: Regex,
    pub(super) layout: OwnedValidatedRowLayout,
    pub(super) local_residual: Option<Program<'statement>>,
}

impl ScanPlan<'_> {
    pub(crate) const fn regex(&self) -> &Regex {
        &self.regex
    }

    pub(crate) fn row_layout(&self) -> RowLayout<'_> {
        self.layout.row_layout()
    }

    #[cfg(test)]
    pub(crate) fn validated_row_layout(&self) -> ValidatedRowLayout<'_> {
        self.layout.validated_row_layout()
    }

    pub(crate) const fn local_residual(&self) -> Option<&Program<'_>> {
        self.local_residual.as_ref()
    }
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
