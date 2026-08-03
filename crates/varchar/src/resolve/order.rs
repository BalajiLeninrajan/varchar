//! Source-column resolution for `ORDER BY` terms.

use super::column::{ColumnLocation, resolve_column};
use crate::sql::{OrderDirection, OrderTerm};
use crate::storage::TableSchema;
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedOrderTerm {
    pub(crate) column: ColumnLocation,
    pub(crate) descending: bool,
}

pub(super) fn resolve_order(
    schemas: &[&TableSchema],
    terms: &[OrderTerm],
) -> Result<Vec<ResolvedOrderTerm>> {
    let mut resolved = Vec::new();
    resolved
        .try_reserve_exact(terms.len())
        .map_err(|_| Error::Allocation {
            operation: "reserving resolved ORDER BY terms",
        })?;
    for term in terms {
        resolved.push(ResolvedOrderTerm {
            column: resolve_column(schemas, &term.column)?,
            descending: matches!(term.direction, OrderDirection::Descending),
        });
    }
    Ok(resolved)
}
