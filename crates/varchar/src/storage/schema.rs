//! Table schemas and the physical row layouts they define.

use std::collections::BTreeSet;

use super::format;
use crate::{DataType, Error, Result, SchemaColumn, Value};

/// The physical shape required to encode, decode, or scan one table's rows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowLayout<'a> {
    pub(crate) table: &'a str,
    pub(crate) columns: &'a [SchemaColumn],
}

/// A table definition reconstructed from its schema and key metadata records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TableSchema {
    pub(crate) name: String,
    pub(crate) columns: Vec<SchemaColumn>,
    pub(crate) primary_key: Option<usize>,
    /// Non-primary single-column UNIQUE constraints in column order.
    pub(crate) unique_columns: Vec<usize>,
    /// Increasing by local column; each local column appears at most once.
    pub(crate) foreign_keys: Vec<ForeignKey>,
}

impl TableSchema {
    pub(crate) fn row_layout(&self) -> RowLayout<'_> {
        RowLayout {
            table: &self.name,
            columns: &self.columns,
        }
    }
}

/// A single-column foreign key reconstructed from schema metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ForeignKey {
    /// Index of the referencing column in the local table.
    pub(crate) column: usize,
    pub(crate) referenced_table: String,
    pub(crate) referenced_column: String,
}

pub(crate) fn validate_schema_for_write(schema: &TableSchema) -> Result<()> {
    validate_row_layout(schema.row_layout())?;

    if let Some(primary_key) = schema.primary_key {
        let Some(column) = schema.columns.get(primary_key) else {
            return Err(Error::Schema(format!(
                "primary-key index {primary_key} is outside table {:?}",
                schema.name
            )));
        };
        if column.nullable {
            return Err(Error::Schema(format!(
                "primary-key column {:?}.{:?} must be NOT NULL",
                schema.name, column.name
            )));
        }
    }

    let mut previous_unique = None;
    for &unique in &schema.unique_columns {
        let Some(column) = schema.columns.get(unique) else {
            return Err(Error::Schema(format!(
                "UNIQUE index {unique} is outside table {:?}",
                schema.name
            )));
        };
        if schema.primary_key == Some(unique) {
            return Err(Error::Schema(format!(
                "primary-key column {:?}.{:?} must not retain redundant UNIQUE metadata",
                schema.name, column.name
            )));
        }
        if previous_unique.is_some_and(|previous| previous >= unique) {
            return Err(Error::Schema(format!(
                "UNIQUE columns for table {:?} must be strictly increasing",
                schema.name
            )));
        }
        previous_unique = Some(unique);
    }

    for column in &schema.columns {
        let Some(default) = &column.default else {
            continue;
        };
        let valid = match (default, column.data_type) {
            (Value::Null, _) => column.nullable,
            (Value::Text(_), DataType::Text)
            | (Value::Integer(_), DataType::Integer)
            | (Value::Boolean(_), DataType::Boolean) => true,
            _ => false,
        };
        if !valid {
            return Err(Error::Schema(format!(
                "invalid DEFAULT for column {:?}.{:?}",
                schema.name, column.name
            )));
        }
    }

    let mut previous_foreign_key_column = None;
    for foreign_key in &schema.foreign_keys {
        if schema.columns.get(foreign_key.column).is_none() {
            return Err(Error::Schema(format!(
                "foreign-key index {} is outside table {:?}",
                foreign_key.column, schema.name
            )));
        }
        if let Some(previous) = previous_foreign_key_column {
            if foreign_key.column == previous {
                return Err(Error::Schema(format!(
                    "column {:?}.{:?} has multiple foreign keys",
                    schema.name, schema.columns[foreign_key.column].name
                )));
            }
            if foreign_key.column < previous {
                return Err(Error::Schema(format!(
                    "foreign keys for table {:?} are not in increasing local-column order",
                    schema.name
                )));
            }
        }
        previous_foreign_key_column = Some(foreign_key.column);
        if !format::is_valid_identifier(&foreign_key.referenced_table)
            || !format::is_valid_identifier(&foreign_key.referenced_column)
        {
            return Err(Error::Schema(format!(
                "invalid foreign-key target {:?}.{:?}",
                foreign_key.referenced_table, foreign_key.referenced_column
            )));
        }
    }
    Ok(())
}

pub(crate) fn validate_row_layout(layout: RowLayout<'_>) -> Result<()> {
    if !format::is_valid_identifier(layout.table) {
        return Err(Error::Schema(format!(
            "invalid or noncanonical table name {:?}",
            layout.table
        )));
    }
    if layout.columns.is_empty() {
        return Err(Error::Schema(String::from(
            "table must contain at least one column",
        )));
    }
    let mut names = BTreeSet::new();
    for column in layout.columns {
        if !format::is_valid_identifier(&column.name) {
            return Err(Error::Schema(format!(
                "invalid or noncanonical column name {:?}",
                column.name
            )));
        }
        if !names.insert(column.name.as_str()) {
            return Err(Error::Schema(format!(
                "duplicate column name {:?}",
                column.name
            )));
        }
    }
    Ok(())
}
