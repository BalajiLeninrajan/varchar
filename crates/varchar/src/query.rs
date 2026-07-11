//! Query planning, physical compilation, and row execution.

mod compile;
mod execute;

use fancy_regex::Regex;

use crate::limits::{Limits, check_limit};
use crate::resolve::{self, ColumnLocation, ResolvedJoin};
use crate::sql::{Predicate, Select};
use crate::storage::{Catalog, TableSchema};
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
    pub(crate) fn into_regex_plan(self) -> RegexPlan {
        let Self {
            pattern,
            regex: _,
            sources,
            projection,
            joins: _,
        } = self;
        let table = sources
            .first()
            .expect("a SELECT plan always has a root source")
            .table
            .clone();
        let mut source_offsets = Vec::with_capacity(sources.len());
        let mut schema = Vec::new();
        for source in sources {
            source_offsets.push(schema.len());
            schema.extend(source.schema);
        }
        let projection = projection
            .into_iter()
            .map(|location| source_offsets[location.source] + location.column)
            .collect();

        RegexPlan {
            pattern,
            table,
            schema,
            projection,
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
