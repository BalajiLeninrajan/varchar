//! Compilation of resolved queries and predicates into executable scan plans.

use fancy_regex::{Regex, RegexBuilder};

use super::{ScanPlan, SelectPlan, pattern, pushdown};
use crate::limits::Limits;
use crate::resolve::{self, ResolvedSelect};
use crate::sql::{Expression, Select};
use crate::storage::{Catalog, TableSchema};
use crate::{Error, Result};

pub(crate) fn compile_select<'catalog, 'statement>(
    catalog: &'catalog Catalog,
    statement: &'statement Select,
    limits: &Limits,
) -> Result<SelectPlan<'catalog, 'statement>> {
    let ResolvedSelect {
        sources,
        projection,
        joins,
        where_clause,
        order_by,
        limit,
        offset,
    } = resolve::select(
        catalog,
        statement,
        limits.max_join_sources,
        limits.max_predicates,
        limits.max_query_output_bytes,
    )?;

    let partition = pushdown::partition(where_clause, sources.len())?;
    let pattern = if sources.len() == 1 {
        pattern::row_scan_pattern(
            &sources[0].name,
            &sources[0].columns,
            &partition.regex_by_source[0],
            limits.max_pattern_bytes,
        )?
    } else {
        pattern::alternate_source_patterns(
            sources
                .iter()
                .zip(&partition.regex_by_source)
                .map(|(schema, predicates)| {
                    pattern::row_scan_pattern(
                        &schema.name,
                        &schema.columns,
                        predicates,
                        limits.max_pattern_bytes,
                    )
                }),
            limits.max_pattern_bytes,
        )?
    };
    let regex = build_regex(&pattern, limits)?;

    Ok(SelectPlan {
        pattern,
        regex,
        sources,
        projection,
        joins,
        local_residuals: partition.local_residuals,
        cross_source_residual: partition.cross_source_residual,
        order_by,
        limit,
        offset,
    })
}

pub(crate) fn compile_scan<'statement>(
    schema: &TableSchema,
    where_clause: Option<&'statement Expression>,
    limits: &Limits,
) -> Result<ScanPlan<'statement>> {
    let where_clause = resolve::local_expression(schema, where_clause, limits.max_predicates)?;
    let mut partition = pushdown::partition(where_clause, 1)?;
    let predicates = partition.regex_by_source.pop().ok_or(Error::Capacity {
        operation: "taking a local scan predicate bucket",
    })?;
    let local_residual = partition.local_residuals.pop().ok_or(Error::Capacity {
        operation: "taking a local scan residual program",
    })?;
    if partition.cross_source_residual.is_some() {
        return Err(Error::Capacity {
            operation: "compiling a cross-source mutation residual",
        });
    }
    let pattern = pattern::row_scan_pattern(
        &schema.name,
        &schema.columns,
        &predicates,
        limits.max_pattern_bytes,
    )?;
    // Compile eagerly so public planning never returns an unusable pattern.
    let regex = build_regex(&pattern, limits)?;
    Ok(ScanPlan {
        regex,
        table: schema.name.clone(),
        schema: schema.columns.clone(),
        local_residual,
    })
}

fn build_regex(pattern: &str, limits: &Limits) -> Result<Regex> {
    let mut builder = RegexBuilder::new(pattern);
    builder
        .backtrack_limit(limits.regex_backtrack_limit)
        .delegate_size_limit(limits.max_pattern_bytes);
    builder
        .build()
        .map_err(|error| Error::RegexCompile(error.to_string()))
}
