//! Compilation of resolved queries and predicates into executable scan plans.

use fancy_regex::{Regex, RegexBuilder};

use super::{ScanPlan, SelectPlan, pattern};
use crate::expression::{Predicate, Program, ProgramNode};
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
    } = resolve::select(
        catalog,
        statement,
        limits.max_join_sources,
        limits.max_predicates,
        limits.max_query_output_bytes,
    )?;

    let routed = route(where_clause, sources.len())?;
    let pattern = if sources.len() == 1 {
        pattern::row_scan_pattern(
            &sources[0].name,
            &sources[0].columns,
            &routed.regex_by_source[0],
            limits.max_pattern_bytes,
        )?
    } else {
        pattern::alternate_source_patterns(
            sources
                .iter()
                .zip(&routed.regex_by_source)
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
        local_residuals: routed.local_residuals,
        cross_source_residual: routed.cross_source_residual,
    })
}

pub(crate) fn compile_scan<'statement>(
    schema: &TableSchema,
    where_clause: Option<&'statement Expression>,
    limits: &Limits,
) -> Result<ScanPlan<'statement>> {
    let where_clause = resolve::local_expression(schema, where_clause, limits.max_predicates)?;
    let mut routed = route(where_clause, 1)?;
    let predicates = routed.regex_by_source.pop().ok_or(Error::Capacity {
        operation: "taking a local scan predicate bucket",
    })?;
    let local_residual = routed.local_residuals.pop().ok_or(Error::Capacity {
        operation: "taking a local scan residual program",
    })?;
    if routed.cross_source_residual.is_some() {
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

/// Where each part of a resolved `WHERE` program is executed.
struct Routing<'statement> {
    regex_by_source: Vec<Vec<Predicate<'statement>>>,
    local_residuals: Vec<Option<Program<'statement>>>,
    cross_source_residual: Option<Program<'statement>>,
}

/// Send a resolved program either wholly into the scan pattern or wholly into a
/// residual slot.
///
/// A conjunction of leaves is exactly what the scan pattern expresses, so its
/// leaves are bucketed by source as before. Anything else is evaluated in Rust:
/// against one source's decoded rows when every leaf resolves there, and
/// against joined rows otherwise.
fn route<'statement>(
    program: Option<Program<'statement>>,
    sources: usize,
) -> Result<Routing<'statement>> {
    let mut regex_by_source = Vec::new();
    regex_by_source
        .try_reserve_exact(sources)
        .map_err(|_| Error::Allocation {
            operation: "reserving query predicate buckets",
        })?;
    regex_by_source.resize_with(sources, Vec::new);
    let mut local_residuals = Vec::new();
    local_residuals
        .try_reserve_exact(sources)
        .map_err(|_| Error::Allocation {
            operation: "reserving query residual programs",
        })?;
    local_residuals.resize_with(sources, || None);
    let mut cross_source_residual = None;

    if let Some(program) = program {
        if is_conjunction_of_leaves(program.nodes()) {
            for node in program.into_nodes() {
                if let ProgramNode::Predicate(predicate) = node {
                    let location = predicate.column();
                    let bucket =
                        regex_by_source
                            .get_mut(location.source)
                            .ok_or(Error::Capacity {
                                operation: "bucketing a resolved predicate by source",
                            })?;
                    bucket.push(predicate);
                }
            }
        } else if let Some(source) = single_source(program.nodes()) {
            let slot = local_residuals.get_mut(source).ok_or(Error::Capacity {
                operation: "routing a source-local residual program",
            })?;
            *slot = Some(program);
        } else {
            cross_source_residual = Some(program);
        }
    }

    Ok(Routing {
        regex_by_source,
        local_residuals,
        cross_source_residual,
    })
}

/// Report whether every leaf of the program is a top-level conjunct.
fn is_conjunction_of_leaves(nodes: &[ProgramNode<'_>]) -> bool {
    match nodes.split_first() {
        None => true,
        Some((ProgramNode::Predicate(_), rest)) => rest.is_empty(),
        Some((ProgramNode::And { .. }, rest)) => rest
            .iter()
            .all(|node| matches!(node, ProgramNode::Predicate(_))),
    }
}

/// Report the single source every leaf of the program reads, if there is one.
fn single_source(nodes: &[ProgramNode<'_>]) -> Option<usize> {
    let mut only = None;
    for node in nodes {
        if let ProgramNode::Predicate(predicate) = node {
            let source = predicate.column().source;
            match only {
                None => only = Some(source),
                Some(seen) if seen == source => {}
                Some(_) => return None,
            }
        }
    }
    only
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
