//! Query planning, physical compilation, and row execution.

mod compile;
mod execute;

use fancy_regex::Regex;

use crate::limits::{Limits, check_limit};
use crate::resolve;
use crate::sql::{Predicate, Select};
use crate::storage::{Candidate, Catalog, TableSchema};
use crate::{Column, Result, RowSet, Value};

/// The exact regular expression and projection produced for a `SELECT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegexPlan {
    pattern: String,
    table: String,
    schema: Vec<Column>,
    projection: Vec<usize>,
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

pub(crate) struct ScanPlan {
    pattern: String,
    regex: Regex,
    table: String,
    schema: Vec<Column>,
}

pub(crate) struct SelectPlan {
    scan: ScanPlan,
    projection: Vec<usize>,
}

impl SelectPlan {
    pub(crate) fn into_regex_plan(self) -> RegexPlan {
        RegexPlan {
            pattern: self.scan.pattern,
            table: self.scan.table,
            schema: self.scan.schema,
            projection: self.projection,
        }
    }
}

pub(crate) fn compile_select(
    catalog: &Catalog,
    statement: &Select,
    limits: &Limits,
) -> Result<SelectPlan> {
    let schema = resolve::require_table(catalog, &statement.table)?;
    let projection = resolve::projection(schema, &statement.projection)?;
    let scan = compile_scan(schema, &statement.predicates, limits)?;
    Ok(SelectPlan { scan, projection })
}

pub(crate) fn compile_scan(
    schema: &TableSchema,
    predicates: &[Predicate],
    limits: &Limits,
) -> Result<ScanPlan> {
    check_limit(predicates.len(), limits.max_predicates, "WHERE predicates")?;
    let resolved = predicates
        .iter()
        .map(|predicate| resolve::predicate(schema, predicate));
    compile::scan(schema, resolved, limits)
}

pub(crate) fn execute_select(blob: &str, plan: &SelectPlan, limits: &Limits) -> Result<RowSet> {
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
