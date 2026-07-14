//! Canonical serialization for schemas, rows, and typed cells.

use super::format::{SCHEMA_PREFIX, encode_text_into, type_tag};
use super::{RowLayout, TableSchema, validate_row_layout, validate_schema_for_write};
use crate::value::validate_value;
use crate::{Column, DataType, Error, Result, Value};

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
    Ok(encoded)
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
pub(crate) fn encode_cell(value: &Value, column: &Column) -> Result<String> {
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
