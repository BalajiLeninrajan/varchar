//! Catalog table lookup with stable schema errors.

use crate::storage::{Catalog, TableSchema};
use crate::{Error, Result};

pub(crate) fn require_table<'a>(catalog: &'a Catalog, table: &str) -> Result<&'a TableSchema> {
    catalog
        .table(table)
        .ok_or_else(|| Error::schema(format!("unknown table {table:?}")))
}
