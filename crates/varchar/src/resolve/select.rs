//! Semantic orchestration for multi-source `SELECT` statements.

use super::column::ColumnLocation;
use super::expression::expression as resolve_expression;
use super::join::{ResolvedJoin, resolve_joins};
use super::projection::{expanded_len, resolve_projection};
use super::source::resolve_sources;
use crate::Result;
use crate::expression::Program;
use crate::sql::Select;
use crate::storage::{Catalog, TableSchema};

pub(crate) struct ResolvedSelect<'catalog, 'statement> {
    pub(crate) sources: Vec<&'catalog TableSchema>,
    pub(crate) projection: Vec<ColumnLocation>,
    pub(crate) joins: Vec<ResolvedJoin>,
    pub(crate) where_clause: Option<Program<'statement>>,
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
    let where_clause =
        resolve_expression(&sources, statement.where_clause.as_ref(), max_predicates)?;
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
        where_clause,
    })
}
