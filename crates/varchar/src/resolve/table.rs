//! Catalog table lookup with stable schema errors.

use crate::storage::{Catalog, TableSchema, ValidatedTableSchema};
use crate::{Error, Result};

pub(crate) fn require_table<'a>(catalog: &'a Catalog, table: &str) -> Result<&'a TableSchema> {
    catalog
        .table(table)
        .ok_or_else(|| Error::Schema(format!("unknown table {table:?}")))
}

pub(crate) fn require_validated_table<'a>(
    catalog: &'a Catalog,
    table: &str,
) -> Result<ValidatedTableSchema<'a>> {
    catalog
        .validated_table(table)
        .ok_or_else(|| Error::Schema(format!("unknown table {table:?}")))
}
