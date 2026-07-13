//! Query planning, physical compilation, and row execution.

mod compile;
mod execute;

use fancy_regex::Regex;

use crate::limits::{Limits, check_limit};
use crate::resolve::{self, ColumnLocation, ResolvedJoin};
use crate::sql::{Predicate, Select};
use crate::storage::{Catalog, TableSchema};
use crate::{Column, ColumnOrigin, Result, ResultColumn, RowSet, Value};

/// An explanation of the source-row scan produced for a `SELECT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExplainPlan {
    pattern: String,
    sources: Vec<String>,
    columns: Vec<ResultColumn>,
}

impl ExplainPlan {
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

pub(crate) struct ScanPlan {
    regex: Regex,
    table: String,
    schema: Vec<Column>,
}

struct SelectSource {
    table: String,
    schema: Vec<Column>,
}

pub(crate) struct SelectPlan {
    pattern: String,
    regex: Regex,
    sources: Vec<SelectSource>,
    projection: Vec<ColumnLocation>,
    joins: Vec<ResolvedJoin>,
}

impl SelectPlan {
    pub(crate) fn into_explain_plan(self) -> ExplainPlan {
        let Self {
            pattern,
            regex: _,
            sources,
            projection,
            joins: _,
        } = self;
        let columns = projection
            .into_iter()
            .map(|location| {
                let source = &sources[location.source];
                let column = &source.schema[location.column];
                ResultColumn::new(
                    column.name.clone(),
                    ColumnOrigin::new(source.table.clone(), column.name.clone()),
                    column.data_type,
                    column.nullable,
                )
            })
            .collect();
        let sources = sources.into_iter().map(|source| source.table).collect();

        ExplainPlan {
            pattern,
            sources,
            columns,
        }
    }
}

pub(crate) fn compile_select(
    catalog: &Catalog,
    statement: &Select,
    limits: &Limits,
) -> Result<SelectPlan> {
    let resolved = resolve::select(
        catalog,
        statement,
        limits.max_join_sources,
        limits.max_predicates,
    )?;
    compile::select(resolved, limits)
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
    blob: &str,
    plan: &ScanPlan,
    limits: &Limits,
    rewrite: F,
) -> Result<(String, usize)>
where
    F: FnMut(Vec<Value>) -> Result<Option<Vec<Value>>>,
{
    execute::rewrite_matching_rows(blob, plan, limits, rewrite)
}
