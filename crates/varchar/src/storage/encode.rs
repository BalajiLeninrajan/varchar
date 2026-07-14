//! Canonical serialization for schemas, rows, and typed cells.

use super::format::{
    AUTO_INCREMENT_PREFIX, FOREIGN_KEY_PREFIX, PRIMARY_KEY_PREFIX, SCHEMA_PREFIX, encode_text_into,
    type_tag,
};
use super::{RowLayout, TableSchema, validate_row_layout, validate_schema_for_write};
use crate::value::validate_value;
use crate::{DataType, Error, Result, SchemaColumn, Value};

/// Encode a complete schema record, including its terminator.
pub(crate) fn encode_schema(schema: &TableSchema) -> Result<String> {
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

    // Foreign-key order is not semantically meaningful. Encoding by local
    // column keeps the authoritative string deterministic.
    for column in 0..schema.columns.len() {
        let Some(foreign_key) = schema
            .foreign_keys
            .iter()
            .find(|foreign_key| foreign_key.column == column)
        else {
            continue;
        };
        encoded.push_str(FOREIGN_KEY_PREFIX);
        encoded.push_str(&schema.name);
        encoded.push('|');
        encoded.push_str(&schema.columns[column].name);
        encoded.push('|');
        encoded.push_str(&foreign_key.referenced_table);
        encoded.push('|');
        encoded.push_str(&foreign_key.referenced_column);
        encoded.push(';');
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

/// Encode a complete row record, including its terminator.
pub(crate) fn encode_row(values: &[Value], layout: RowLayout<'_>) -> Result<String> {
    validate_row_layout(layout)?;
    if values.len() != layout.columns.len() {
        return Err(Error::Type(format!(
            "table {:?} expects {} values, got {}",
            layout.table,
            layout.columns.len(),
            values.len()
        )));
    }

    let mut encoded = String::from("~R|");
    encoded.push_str(layout.table);
    for (value, column) in values.iter().zip(layout.columns) {
        encoded.push('|');
        encoded.push_str(&encode_cell(value, column)?);
    }
    encoded.push(';');
    Ok(encoded)
}

/// Encode one typed cell in its canonical storage representation.
pub(crate) fn encode_cell(value: &Value, column: &SchemaColumn) -> Result<String> {
    validate_value(value, column)?;
    match (value, column.data_type) {
        (Value::Null, _) => Ok(String::from("N")),
        (Value::Text(value), DataType::Text) => {
            let mut encoded = String::from("T");
            encode_text_into(value, &mut encoded);
            Ok(encoded)
        }
        (Value::Integer(value), DataType::Integer) => Ok(format!("I{value}")),
        (Value::Boolean(value), DataType::Boolean) => {
            Ok(String::from(if *value { "B1" } else { "B0" }))
        }
        _ => unreachable!("value validation guarantees the encoded type"),
    }
}
