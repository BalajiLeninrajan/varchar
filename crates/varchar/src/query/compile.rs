//! Compilation of resolved queries and predicates into executable scan plans.

use fancy_regex::{Regex, RegexBuilder};

use super::{ScanPlan, SelectPlan, pattern};
use crate::limits::{Limits, check_limit};
use crate::resolve::{self, ResolvedSelect};
use crate::sql::{Predicate, Select};
use crate::storage::{Catalog, TableSchema};
use crate::{Error, Resource, Result};

pub(crate) fn compile_select<'catalog>(
    catalog: &'catalog Catalog,
    statement: &Select,
    limits: &Limits,
) -> Result<SelectPlan<'catalog>> {
    let ResolvedSelect {
        sources,
        projection,
        joins,
        predicates,
    } = resolve::select(
        catalog,
        statement,
        limits.max_join_sources,
        limits.max_predicates,
        limits.max_query_output_bytes,
    )?;

    let mut predicates_by_source = Vec::new();
    predicates_by_source
        .try_reserve_exact(sources.len())
        .map_err(|_| Error::Allocation {
            operation: "reserving query predicate buckets",
        })?;
    predicates_by_source.resize_with(sources.len(), Vec::new);
    for resolved in predicates {
        predicates_by_source[resolved.column().source].push(resolved);
    }

    let pattern = if sources.len() == 1 {
        pattern::row_scan_pattern(
            &sources[0].name,
            &sources[0].columns,
            &predicates_by_source[0],
            limits.max_pattern_bytes,
        )?
    } else {
        pattern::alternate_source_patterns(
            sources
                .iter()
                .zip(&predicates_by_source)
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
    })
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
    let predicates = predicates
        .iter()
        .map(|predicate| resolve::predicate(schema, predicate))
        .collect::<Result<Vec<_>>>()?;
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
