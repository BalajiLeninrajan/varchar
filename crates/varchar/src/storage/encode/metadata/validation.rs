use crate::storage::TableSchema;
use crate::storage::format::is_valid_identifier;
use crate::{DataType, Error, Result, Value};

pub(super) fn validate_table_metadata(
    schema: &TableSchema,
    auto_increment: Option<(usize, i64)>,
) -> Result<()> {
    validate_schema_for_metadata(schema)?;
    if let Some((column, last)) = auto_increment {
        validate_auto_increment_record(schema, column, last)?;
    }
    Ok(())
}

fn validate_schema_for_metadata(schema: &TableSchema) -> Result<()> {
    if !is_valid_identifier(&schema.name) {
        return Err(Error::Schema(format!(
            "invalid or noncanonical table name {:?}",
            schema.name
        )));
    }
    if schema.columns.is_empty() {
        return Err(Error::Schema(String::from(
            "table must contain at least one column",
        )));
    }
    for (index, column) in schema.columns.iter().enumerate() {
        if !is_valid_identifier(&column.name) {
            return Err(Error::Schema(format!(
                "invalid or noncanonical column name {:?}",
                column.name
            )));
        }
        if schema.columns[..index]
            .iter()
            .any(|previous| previous.name == column.name)
        {
            return Err(Error::Schema(format!(
                "duplicate column name {:?}",
                column.name
            )));
        }
    }

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

    if !schema.checks.is_empty() {
        return Err(Error::Schema(String::from(
            "CHECK metadata requires a persisted program",
        )));
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
        if !is_valid_identifier(&foreign_key.referenced_table)
            || !is_valid_identifier(&foreign_key.referenced_column)
        {
            return Err(Error::Schema(format!(
                "invalid foreign-key target {:?}.{:?}",
                foreign_key.referenced_table, foreign_key.referenced_column
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_auto_increment_record(
    schema: &TableSchema,
    column: usize,
    last: i64,
) -> Result<()> {
    if last < 0 {
        return Err(Error::Schema(format!(
            "auto-increment high-water mark for table {:?} must be nonnegative",
            schema.name
        )));
    }
    let Some(definition) = schema.columns.get(column) else {
        return Err(Error::Schema(format!(
            "auto-increment index {column} is outside table {:?}",
            schema.name
        )));
    };
    if schema.primary_key != Some(column) || definition.data_type != DataType::Integer {
        return Err(Error::Schema(format!(
            "auto-increment column {:?}.{:?} must be its INTEGER primary key",
            schema.name, definition.name
        )));
    }
    Ok(())
}
