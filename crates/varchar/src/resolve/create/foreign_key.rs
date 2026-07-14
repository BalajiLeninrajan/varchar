//! Foreign-key declaration and referenced-schema validation.

use crate::storage::{Catalog, ForeignKey, TableSchema};
use crate::{Error, Result};

pub(super) fn declare_foreign_key(
    column: &str,
    syntax: &str,
    index: usize,
    referenced_table: String,
    referenced_column: String,
    saw_foreign_key: &mut [bool],
    foreign_keys: &mut Vec<ForeignKey>,
) -> Result<()> {
    if saw_foreign_key[index] {
        return Err(Error::schema(format!(
            "duplicate {syntax} declaration for column {column:?}"
        )));
    }
    saw_foreign_key[index] = true;
    foreign_keys.push(ForeignKey {
        column: index,
        referenced_table,
        referenced_column,
    });
    Ok(())
}

pub(super) fn validate_foreign_key(
    catalog: &Catalog,
    schema: &TableSchema,
    foreign_key: &ForeignKey,
) -> Result<()> {
    let referenced_schema = if foreign_key.referenced_table == schema.name {
        schema
    } else {
        catalog
            .table(&foreign_key.referenced_table)
            .ok_or_else(|| {
                Error::schema(format!(
                    "foreign key references unknown or later table {:?}",
                    foreign_key.referenced_table
                ))
            })?
    };
    let referenced_primary_key = referenced_schema
        .primary_key
        .filter(|&index| referenced_schema.columns[index].name == foreign_key.referenced_column);
    let Some(referenced_primary_key) = referenced_primary_key else {
        return Err(Error::schema(format!(
            "foreign key target {:?}.{:?} is not its table's primary key",
            foreign_key.referenced_table, foreign_key.referenced_column
        )));
    };
    if schema.columns[foreign_key.column].data_type
        != referenced_schema.columns[referenced_primary_key].data_type
    {
        return Err(Error::schema(format!(
            "foreign-key columns {:?}.{:?} and {:?}.{:?} have different types",
            schema.name,
            schema.columns[foreign_key.column].name,
            foreign_key.referenced_table,
            foreign_key.referenced_column
        )));
    }
    Ok(())
}
