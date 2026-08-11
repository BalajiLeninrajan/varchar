//! Canonical schema and constraint metadata encoding.

use super::super::format::{
    AUTO_INCREMENT_PREFIX, DEFAULT_PREFIX, FOREIGN_KEY_PREFIX, PRIMARY_KEY_PREFIX, SCHEMA_PREFIX,
    type_tag,
};
use super::super::{TableSchema, validate_schema_for_write};
use super::encode_cell;
use crate::{DataType, Error, Result};

/// Encode a schema and its key records, preserving the frozen V2 byte shape.
fn encode_schema(schema: &TableSchema) -> Result<String> {
    validate_schema_for_write(schema)?;

    let mut encoded = String::from(SCHEMA_PREFIX);
    encoded.push_str(&schema.name);
    for column in &schema.columns {
        encoded.push('|');
        encoded.push_str(&column.name);
        encoded.push(':');
        encoded.push(type_tag(column.data_type));
        encoded.push(':');
        encoded.push(if column.nullable { '?' } else { '!' });
    }
    encoded.push(';');

    if let Some(primary_key) = schema.primary_key {
        encoded.push_str(PRIMARY_KEY_PREFIX);
        encoded.push_str(&schema.name);
        encoded.push('|');
        encoded.push_str(&schema.columns[primary_key].name);
        encoded.push(';');
    }

    // Resolved schemas and decoded metadata retain foreign keys in increasing
    // local-column order, so direct iteration preserves canonical encoding.
    for foreign_key in &schema.foreign_keys {
        encoded.push_str(FOREIGN_KEY_PREFIX);
        encoded.push_str(&schema.name);
        encoded.push('|');
        encoded.push_str(&schema.columns[foreign_key.column].name);
        encoded.push('|');
        encoded.push_str(&foreign_key.referenced_table);
        encoded.push('|');
        encoded.push_str(&foreign_key.referenced_column);
        encoded.push(';');
    }
    Ok(encoded)
}

/// Encode all metadata for one newly created table in canonical phase order.
pub(in crate::storage) fn encode_table_metadata(
    schema: &TableSchema,
    auto_increment: Option<(usize, i64)>,
) -> Result<String> {
    let mut encoded = encode_schema(schema)?;
    if let Some((column, last)) = auto_increment {
        encoded.push_str(&encode_auto_increment_record(schema, column, last)?);
    }
    for (column, definition) in schema.columns.iter().enumerate() {
        if definition.default.is_some() {
            encoded.push_str(&encode_default_record(schema, column)?);
        }
    }
    Ok(encoded)
}

/// Encode one persisted auto-increment high-water record.
pub(crate) fn encode_auto_increment_record(
    schema: &TableSchema,
    column: usize,
    last: i64,
) -> Result<String> {
    validate_schema_for_write(schema)?;
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
    Ok(format!(
        "{AUTO_INCREMENT_PREFIX}{}|{}|I{last};",
        schema.name, definition.name
    ))
}

fn encode_default_record(schema: &TableSchema, column: usize) -> Result<String> {
    let definition = schema.columns.get(column).ok_or_else(|| {
        Error::Schema(format!(
            "DEFAULT index {column} is outside table {:?}",
            schema.name
        ))
    })?;
    let value = definition.default.as_ref().ok_or_else(|| {
        Error::Schema(format!(
            "column {:?}.{:?} has no DEFAULT",
            schema.name, definition.name
        ))
    })?;
    let cell = encode_cell(value, definition)?;
    Ok(format!(
        "{DEFAULT_PREFIX}{}|{}|{cell};",
        schema.name, definition.name
    ))
}
