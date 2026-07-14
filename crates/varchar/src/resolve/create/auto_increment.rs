//! Auto-increment declaration and schema rules.

use crate::storage::TableSchema;
use crate::{DataType, Error, Result};

pub(super) fn declare_auto_increment(
    table: &str,
    column: &str,
    index: usize,
    auto_increment: &mut Option<usize>,
) -> Result<()> {
    match *auto_increment {
        Some(existing) if existing == index => Err(Error::schema(format!(
            "duplicate AUTOINCREMENT declaration for column {column:?}"
        ))),
        Some(_) => Err(Error::schema(format!(
            "table {table:?} may have only one auto-increment column"
        ))),
        None => {
            *auto_increment = Some(index);
            Ok(())
        }
    }
}

pub(super) fn validate_auto_increment(schema: &TableSchema, column: usize) -> Result<()> {
    let definition = &schema.columns[column];
    if schema.primary_key != Some(column) || definition.data_type != DataType::Integer {
        return Err(Error::schema(format!(
            "auto-increment column {:?}.{:?} must be its INTEGER primary key",
            schema.name, definition.name
        )));
    }
    Ok(())
}
