//! Compilation of resolved queries and predicates into executable scan plans.

use fancy_regex::{Regex, RegexBuilder};

use super::{ScanPlan, SelectPlan, pattern};
use crate::expression::{Predicate, Program, ProgramNode};
use crate::limits::Limits;
use crate::resolve::{self, ResolvedSelect};
use crate::sql::{Expression, Select};
use crate::storage::{Catalog, TableSchema};
use crate::{Error, Result};

pub(crate) fn compile_select<'catalog>(
    catalog: &'catalog Catalog,
    statement: &Select,
    limits: &Limits,
) -> Result<SelectPlan<'catalog>> {
    let ResolvedSelect {
        sources,
        projection,
        joins,
        where_clause,
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
    for predicate in leaves(where_clause) {
        let location = predicate.column();
        predicates_by_source[location.source].push(predicate);
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
    where_clause: Option<&Expression>,
    limits: &Limits,
) -> Result<ScanPlan> {
    let where_clause = resolve::local_expression(schema, where_clause, limits.max_predicates)?;
    let predicates = leaves(where_clause);
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

/// Collect the predicate leaves of a resolved `WHERE` program.
///
/// Every expression a `WHERE` can currently hold is a conjunction, so dropping
/// the `And` nodes loses nothing: each leaf must hold for the row to match.
fn leaves(program: Option<Program<'_>>) -> Vec<Predicate<'_>> {
    let Some(program) = program else {
        return Vec::new();
    };
    let mut predicates = Vec::new();
    for node in program.into_nodes() {
        match node {
            ProgramNode::And { .. } => {}
            ProgramNode::Predicate(predicate) => predicates.push(predicate),
        }
    }
    predicates
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
