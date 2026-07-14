//! Resolution and validation of `FROM` and `JOIN` source tables.

use super::table::require_table;
use crate::limits::check_limit;
use crate::sql::Select;
use crate::storage::{Catalog, TableSchema};
use crate::{Error, Result};

pub(super) fn resolve_sources<'catalog>(
    catalog: &'catalog Catalog,
    statement: &Select,
    max_join_sources: usize,
) -> Result<Vec<&'catalog TableSchema>> {
    let source_count = statement
        .joins
        .len()
        .checked_add(1)
        .ok_or(Error::ResourceLimit {
            resource: "JOIN sources",
            limit: max_join_sources,
        })?;
    check_limit(source_count, max_join_sources, "JOIN sources")?;

    let mut sources = Vec::with_capacity(source_count);
    sources.push(require_table(catalog, &statement.table)?);
    for join in &statement.joins {
        if sources.iter().any(|schema| schema.name == join.table) {
            return Err(Error::Schema(format!(
                "table {:?} appears more than once in a SELECT",
                join.table
            )));
        }
        sources.push(require_table(catalog, &join.table)?);
    }
    Ok(sources)
}
