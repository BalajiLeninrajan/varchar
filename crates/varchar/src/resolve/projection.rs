//! Projection-name resolution and bounded expansion across `SELECT` sources.

use super::column::{ColumnLocation, resolve_column};
use crate::limits::check_limit;
use crate::sql::{Projection, ProjectionItem};
use crate::storage::TableSchema;
use crate::{Error, Result};

pub(super) fn expanded_len(schemas: &[&TableSchema], projection: &Projection) -> Result<usize> {
    match projection {
        Projection::All => Ok(schemas.iter().fold(0_usize, |total, schema| {
            total.saturating_add(schema.columns.len())
        })),
        Projection::Items(items) => items.iter().try_fold(0_usize, |total, item| {
            let additional = match item {
                ProjectionItem::Column(column) => {
                    resolve_column(schemas, column)?;
                    1
                }
                ProjectionItem::QualifiedAll(table) => schemas
                    [qualified_projection_source(schemas, table)?]
                .columns
                .len(),
            };
            Ok(total.saturating_add(additional))
        }),
    }
}

pub(super) fn resolve_projection(
    schemas: &[&TableSchema],
    projection: &Projection,
    projection_len: usize,
    max_query_output_bytes: usize,
) -> Result<Vec<ColumnLocation>> {
    let projection_bytes = projection_len
        .checked_mul(std::mem::size_of::<ColumnLocation>())
        .ok_or_else(|| query_output_limit_error(max_query_output_bytes))?;
    check_limit(
        projection_bytes,
        max_query_output_bytes,
        "query output bytes",
    )?;

    let mut resolved = Vec::new();
    resolved
        .try_reserve_exact(projection_len)
        .map_err(|_| query_output_limit_error(max_query_output_bytes))?;
    match projection {
        Projection::All => {
            for (source, schema) in schemas.iter().enumerate() {
                resolved.extend(
                    (0..schema.columns.len()).map(|column| ColumnLocation { source, column }),
                );
            }
        }
        Projection::Items(items) => {
            for item in items {
                match item {
                    ProjectionItem::Column(column) => {
                        resolved.push(resolve_column(schemas, column)?);
                    }
                    ProjectionItem::QualifiedAll(table) => {
                        let source = qualified_projection_source(schemas, table)?;
                        resolved.extend(
                            (0..schemas[source].columns.len())
                                .map(|column| ColumnLocation { source, column }),
                        );
                    }
                }
            }
        }
    }
    debug_assert_eq!(resolved.len(), projection_len);
    Ok(resolved)
}

fn qualified_projection_source(schemas: &[&TableSchema], table: &str) -> Result<usize> {
    schemas
        .iter()
        .position(|schema| schema.name == table)
        .ok_or_else(|| Error::Schema(format!("unknown table qualifier {table:?} in projection")))
}

const fn query_output_limit_error(limit: usize) -> Error {
    Error::ResourceLimit {
        resource: "query output bytes",
        limit,
    }
}
