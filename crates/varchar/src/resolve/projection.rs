//! Projection-name resolution for single-table selects.

use super::column::require_column;
use crate::Result;
use crate::sql::Projection;
use crate::storage::TableSchema;

pub(crate) fn projection(schema: &TableSchema, projection: &Projection) -> Result<Vec<usize>> {
    match projection {
        Projection::All => Ok((0..schema.columns.len()).collect()),
        Projection::Columns(columns) => columns
            .iter()
            .map(|name| require_column(schema, name))
            .collect(),
    }
}
