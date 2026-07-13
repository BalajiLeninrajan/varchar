//! Query planning, physical compilation, and row execution.

mod compile;
mod execute;

use fancy_regex::Regex;

use crate::limits::{Limits, check_limit};
use crate::output::{ColumnOrigin, ResultColumn, RowSet, SelectExplanation};
use crate::resolve::{self, ColumnLocation, ResolvedJoin};
use crate::sql::{Predicate, Select};
use crate::storage::{Candidate, Catalog, TableSchema};
use crate::value::Value;
use crate::{Resource, Result};

pub(crate) struct ScanPlan<'catalog> {
    regex: Regex,
    schema: &'catalog TableSchema,
}

pub(crate) struct SelectPlan<'catalog> {
    pattern: String,
    regex: Regex,
    sources: Vec<&'catalog TableSchema>,
    projection: Vec<ColumnLocation>,
    joins: Vec<ResolvedJoin>,
}

impl SelectPlan<'_> {
    pub(crate) fn into_explanation(self) -> SelectExplanation {
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
                let column = &source.columns[location.column];
                ResultColumn::new(
                    column.name.clone(),
                    ColumnOrigin::new(source.name.clone(), column.name.clone()),
                    column.data_type,
                    column.nullable,
                )
            })
            .collect();
        let sources = sources
            .into_iter()
            .map(|source| source.name.clone())
            .collect();

        SelectExplanation::new(pattern, sources, columns)
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
        limits.max_join_sources(),
        limits.max_predicates(),
    )?;
    compile::select(resolved, limits)
}

pub(crate) fn compile_scan<'catalog>(
    schema: &'catalog TableSchema,
    predicates: &[Predicate],
    limits: &Limits,
) -> Result<ScanPlan<'catalog>> {
    check_limit(
        predicates.len(),
        limits.max_predicates(),
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
    plan: &ScanPlan<'_>,
    limits: &Limits,
    rewrite: F,
) -> Result<usize>
where
    F: FnMut(Vec<Value>) -> Result<Option<Vec<Value>>>,
{
    execute::rewrite_matching_rows(candidate, plan, limits, rewrite)
}

#[cfg(test)]
mod tests {
    use super::{compile_scan, compile_select};
    use crate::Limits;
    use crate::sql::{self, Statement};
    use crate::storage::validate_and_catalog;

    #[test]
    fn private_plans_borrow_catalog_schemas() {
        let catalog =
            validate_and_catalog("V2;~S|items|id:I:!|name:T:?;").expect("fixture catalog is valid");
        let schema = catalog.table("items").expect("fixture table exists");
        let Statement::Select(statement) =
            sql::parse("SELECT name FROM items").expect("fixture SELECT parses")
        else {
            panic!("expected SELECT");
        };

        let select =
            compile_select(&catalog, &statement, &Limits::default()).expect("SELECT plan compiles");
        assert!(std::ptr::eq(select.sources[0], schema));

        let scan = compile_scan(schema, &[], &Limits::default()).expect("scan plan compiles");
        assert!(std::ptr::eq(scan.schema, schema));
    }
}
