//! Query planning, physical compilation, and row execution.

mod compile;
mod execute;
mod pattern;

use fancy_regex::Regex;

use crate::limits::{Limits, check_limit};
use crate::output::{RowSet, SelectExplanation};
use crate::resolve::{self, ColumnLocation, ResolvedJoin};
use crate::sql::{Predicate, Select};
use crate::storage::{Candidate, Catalog, TableSchema};
use crate::value::Value;
use crate::{Resource, Result, SchemaColumn};

/// An owned mutation scan that remains valid while a candidate is assembled.
pub(crate) struct ScanPlan {
    regex: Regex,
    table: String,
    schema: Vec<SchemaColumn>,
}

/// A read-only plan borrowing the catalog schemas used by one `SELECT`.
pub(crate) struct SelectPlan<'catalog> {
    pattern: String,
    regex: Regex,
    sources: Vec<&'catalog TableSchema>,
    projection: Vec<ColumnLocation>,
    joins: Vec<ResolvedJoin>,
}

impl SelectPlan<'_> {
    pub(crate) fn into_explanation(
        self,
        max_query_output_bytes: usize,
    ) -> Result<SelectExplanation> {
        execute::explain(self, max_query_output_bytes)
    }
}

pub(crate) fn compile_select<'catalog>(
    catalog: &'catalog Catalog,
    statement: &Select,
    limits: &Limits,
) -> Result<SelectPlan<'catalog>> {
    let resolved = resolve::select(
        catalog,
        statement,
        limits.max_join_sources,
        limits.max_predicates,
        limits.max_query_output_bytes,
    )?;
    compile::select(resolved, limits)
}

pub(crate) fn compile_scan(
    schema: &TableSchema,
    predicates: &[Predicate],
    limits: &Limits,
) -> Result<ScanPlan> {
    check_limit(
        predicates.len(),
        limits.max_predicates,
        Resource::WherePredicates,
    )?;
    let resolved = predicates
        .iter()
        .map(|predicate| resolve::predicate(schema, predicate));
    compile::scan(schema, resolved, limits)
}

pub(crate) fn execute_select(blob: &str, plan: &SelectPlan<'_>, limits: &Limits) -> Result<RowSet> {
    execute::select(blob, plan, limits)
}

pub(crate) fn rewrite_matching_rows<F>(
    candidate: &mut Candidate<'_>,
    plan: &ScanPlan,
    limits: &Limits,
    rewrite: F,
) -> Result<usize>
where
    F: FnMut(Vec<Value>) -> Result<Option<Vec<Value>>>,
{
    execute::rewrite_matching_rows(candidate, plan, limits, rewrite)
}

#[cfg(test)]
mod tests;
