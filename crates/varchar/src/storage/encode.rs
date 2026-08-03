//! Canonical serialization for metadata, rows, and typed cells.

mod metadata;

pub(super) use metadata::encode_auto_increment_record;
pub(crate) use metadata::encode_schema;

use super::format::encode_text_into;
use super::{RowLayout, validate_row_layout};
use crate::value::validate_value;
use crate::{DataType, Error, Result, SchemaColumn, Value};

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
