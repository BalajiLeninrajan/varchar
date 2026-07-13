use super::format::{SCHEMA_PREFIX, encode_text_into, type_tag};
use super::{TableSchema, validate_schema_for_write};
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
pub(crate) fn encode_row(table: &str, values: &[Value], schema: &TableSchema) -> Result<String> {
    if table != schema.name {
        return Err(Error::Schema(format!(
            "row table {table:?} does not match schema {:?}",
            schema.name
        )));
    }
    validate_schema_for_write(schema)?;
    if values.len() != schema.columns.len() {
        return Err(Error::Type(format!(
            "table {table:?} expects {} values, got {}",
            schema.columns.len(),
            values.len()
        )));
    }

    let mut encoded = String::from("~R|");
    encoded.push_str(table);
    for (value, column) in values.iter().zip(&schema.columns) {
        encoded.push('|');
        encoded.push_str(&encode_cell(value, column)?);
    }
    encoded.push(';');
    Ok(encoded)
}

/// Encode one typed cell in its canonical storage representation.
pub(crate) fn encode_cell(value: &Value, column: &Column) -> Result<String> {
    match (value, column.data_type) {
        (Value::Null, _) if column.nullable => Ok(String::from("N")),
        (Value::Null, _) => Err(Error::Type(format!("column {:?} is NOT NULL", column.name))),
        (Value::Text(value), DataType::Text) => {
            let mut encoded = String::from("T");
            encode_text_into(value, &mut encoded);
            Ok(encoded)
        }
        (Value::Integer(value), DataType::Integer) => Ok(format!("I{value}")),
        (Value::Boolean(value), DataType::Boolean) => {
            Ok(String::from(if *value { "B1" } else { "B0" }))
        }
        (actual, expected) => Err(Error::Type(format!(
            "column {:?} expects {expected}, got {}",
            column.name,
            value_kind(actual)
        ))),
    }
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Text(_) => "TEXT",
        Value::Integer(_) => "INTEGER",
        Value::Boolean(_) => "BOOLEAN",
        Value::Null => "NULL",
    }
}
