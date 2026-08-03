//! Canonical serialization for metadata, rows, and typed cells.

mod metadata;

pub(super) use metadata::{
    encode_auto_increment_record, encode_table_metadata, measure_table_metadata,
};

use std::fmt::Write as _;

use super::format::{allocation_error, encode_text_into, encoded_text_len};
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
    let encoded_len = encoded_cell_len(value, column)?;
    let mut encoded = String::new();
    encoded
        .try_reserve_exact(encoded_len)
        .map_err(|_| allocation_error("reserving an encoded cell"))?;
    encode_cell_into(value, column, &mut encoded)?;
    debug_assert_eq!(encoded.len(), encoded_len);
    Ok(encoded)
}

fn encoded_cell_len(value: &Value, column: &SchemaColumn) -> Result<usize> {
    validate_value(value, column)?;
    match (value, column.data_type) {
        (Value::Null, _) => Ok(1),
        (Value::Text(value), DataType::Text) => {
            encoded_text_len(value)?
                .checked_add(1)
                .ok_or(Error::Capacity {
                    operation: "sizing an encoded TEXT cell",
                })
        }
        (Value::Integer(value), DataType::Integer) => signed_decimal_len(*value)
            .checked_add(1)
            .ok_or(Error::Capacity {
                operation: "sizing an encoded INTEGER cell",
            }),
        (Value::Boolean(_), DataType::Boolean) => Ok(2),
        _ => unreachable!("value validation guarantees the encoded type"),
    }
}

fn encode_cell_into(value: &Value, column: &SchemaColumn, encoded: &mut String) -> Result<()> {
    validate_value(value, column)?;
    match (value, column.data_type) {
        (Value::Null, _) => encoded.push('N'),
        (Value::Text(value), DataType::Text) => {
            encoded.push('T');
            encode_text_into(value, encoded);
        }
        (Value::Integer(value), DataType::Integer) => {
            encoded.push('I');
            // Writing to a String is infallible.
            let _ = write!(encoded, "{value}");
        }
        (Value::Boolean(value), DataType::Boolean) => {
            encoded.push_str(if *value { "B1" } else { "B0" });
        }
        _ => unreachable!("value validation guarantees the encoded type"),
    }
    Ok(())
}

fn signed_decimal_len(value: i64) -> usize {
    let magnitude = value.unsigned_abs();
    let digits = if magnitude == 0 {
        1
    } else {
        magnitude.ilog10() as usize + 1
    };
    digits + usize::from(value.is_negative())
}
