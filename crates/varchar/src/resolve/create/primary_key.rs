//! Primary-key declaration and nullability rules.

use crate::{Error, Result, SchemaColumn};

pub(super) fn declare_primary_key(
    table: &str,
    column: &str,
    index: usize,
    primary_key: &mut Option<usize>,
    columns: &mut [SchemaColumn],
) -> Result<()> {
    match *primary_key {
        Some(existing) if existing == index => {
            return Err(Error::Schema(format!(
                "duplicate PRIMARY KEY declaration for column {column:?}"
            )));
        }
        Some(_) => return Err(multiple_primary_keys(table)),
        None => *primary_key = Some(index),
    }
    columns[index].nullable = false;
    Ok(())
}

fn multiple_primary_keys(table: &str) -> Error {
    Error::Schema(format!(
        "table {table:?} may have only one PRIMARY KEY column"
    ))
}
