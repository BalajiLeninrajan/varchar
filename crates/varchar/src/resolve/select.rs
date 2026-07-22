//! Semantic orchestration for multi-source `SELECT` statements.

use super::column::ColumnLocation;
use super::join::{ResolvedJoin, resolve_joins};
use super::predicate::{ResolvedSourcePredicate, resolve_select_predicate};
use super::projection::{expanded_len, resolve_projection};
use super::source::resolve_sources;
use crate::limits::check_limit;
use crate::sql::Select;
use crate::storage::{Catalog, TableSchema};
use crate::{Resource, Result};

pub(crate) struct ResolvedSelect<'catalog, 'statement> {
    pub(crate) sources: Vec<&'catalog TableSchema>,
    pub(crate) projection: Vec<ColumnLocation>,
    pub(crate) joins: Vec<ResolvedJoin>,
    pub(crate) predicates: Vec<ResolvedSourcePredicate<'statement>>,
}

pub(crate) fn select<'catalog, 'statement>(
    catalog: &'catalog Catalog,
    statement: &'statement Select,
    max_join_sources: usize,
    max_predicates: usize,
    max_query_output_bytes: usize,
) -> Result<ResolvedSelect<'catalog, 'statement>> {
    let sources = resolve_sources(catalog, statement, max_join_sources)?;

    // Resolve every projection name and saturate an overflowing width before
    // applying its allocation bound so semantic errors keep precedence over a
    // resource error.
    let projection_len = expanded_len(&sources, &statement.projection)?;
    let joins = resolve_joins(statement, &sources)?;
    check_limit(
        statement.predicates.len(),
        max_predicates,
        Resource::WherePredicates,
    )?;
    let predicates = statement
        .predicates
        .iter()
        .map(|predicate| resolve_select_predicate(&sources, predicate))
        .collect::<Result<Vec<_>>>()?;
    let projection = resolve_projection(
        &sources,
        &statement.projection,
        projection_len,
        max_query_output_bytes,
    )?;

    Ok(ResolvedSelect {
        sources,
        projection,
        joins,
        predicates,
    })
}
