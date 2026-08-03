//! Canonical schema and constraint metadata encoding.

mod validation;

#[cfg(test)]
mod tests;

use std::fmt::Write as _;

use super::super::format::{
    AUTO_INCREMENT_PREFIX, DEFAULT_PREFIX, FOREIGN_KEY_PREFIX, PRIMARY_KEY_PREFIX, SCHEMA_PREFIX,
    UNIQUE_PREFIX, encode_text_into, encoded_text_len, type_tag,
};
use super::super::{TableSchema, validate_schema_for_write};
use crate::{DataType, Error, Result, Value};

use self::validation::{validate_auto_increment_record, validate_table_metadata};

const METADATA_LENGTH_OPERATION: &str = "measuring encoded table metadata";
const TABLE_METADATA_ALLOCATION: &str = "reserving encoded table metadata";
const AUTO_INCREMENT_ALLOCATION: &str = "reserving encoded auto-increment metadata";

#[derive(Clone, Copy, Debug)]
pub(in crate::storage) struct MeasuredTableMetadata {
    encoded_len: usize,
}

/// Validate metadata without allocation and compute its exact canonical byte length.
pub(in crate::storage) fn measure_table_metadata(
    schema: &TableSchema,
    auto_increment: Option<(usize, i64)>,
) -> Result<MeasuredTableMetadata> {
    validate_table_metadata(schema, auto_increment)?;

    let mut measured = EncodedLength::default();
    stream_table_metadata(schema, auto_increment, &mut measured)?;
    Ok(MeasuredTableMetadata {
        encoded_len: measured.encoded_len,
    })
}

/// Encode all metadata for one newly created table in canonical phase order.
pub(in crate::storage) fn encode_table_metadata(
    schema: &TableSchema,
    auto_increment: Option<(usize, i64)>,
    measured: MeasuredTableMetadata,
) -> Result<String> {
    let mut encoded = String::new();
    encoded
        .try_reserve_exact(measured.encoded_len)
        .map_err(|_| Error::Allocation {
            operation: TABLE_METADATA_ALLOCATION,
        })?;
    stream_table_metadata(schema, auto_increment, &mut EncodedString(&mut encoded))?;
    debug_assert_eq!(encoded.len(), measured.encoded_len);
    Ok(encoded)
}

/// Encode one persisted auto-increment high-water record.
pub(crate) fn encode_auto_increment_record(
    schema: &TableSchema,
    column: usize,
    last: i64,
) -> Result<String> {
    validate_schema_for_write(schema)?;
    validate_auto_increment_record(schema, column, last)?;

    let mut measured = EncodedLength::default();
    stream_auto_increment_record(schema, column, last, &mut measured)?;
    let mut encoded = String::new();
    encoded
        .try_reserve_exact(measured.encoded_len)
        .map_err(|_| Error::Allocation {
            operation: AUTO_INCREMENT_ALLOCATION,
        })?;
    stream_auto_increment_record(schema, column, last, &mut EncodedString(&mut encoded))?;
    debug_assert_eq!(encoded.len(), measured.encoded_len);
    Ok(encoded)
}

trait MetadataSink {
    fn push_str(&mut self, value: &str) -> Result<()>;
    fn push_char(&mut self, value: char) -> Result<()>;
    fn push_text(&mut self, value: &str) -> Result<()>;
    fn push_i64(&mut self, value: i64) -> Result<()>;
}

#[derive(Default)]
struct EncodedLength {
    encoded_len: usize,
}

impl EncodedLength {
    fn add(&mut self, additional: usize) -> Result<()> {
        self.encoded_len = self
            .encoded_len
            .checked_add(additional)
            .ok_or(Error::Capacity {
                operation: METADATA_LENGTH_OPERATION,
            })?;
        Ok(())
    }
}

impl MetadataSink for EncodedLength {
    fn push_str(&mut self, value: &str) -> Result<()> {
        self.add(value.len())
    }

    fn push_char(&mut self, value: char) -> Result<()> {
        self.add(value.len_utf8())
    }

    fn push_text(&mut self, value: &str) -> Result<()> {
        self.add(encoded_text_len(value)?)
    }

    fn push_i64(&mut self, value: i64) -> Result<()> {
        if value < 0 {
            self.add(1)?;
        }
        let mut magnitude = value.unsigned_abs();
        loop {
            self.add(1)?;
            if magnitude < 10 {
                return Ok(());
            }
            magnitude /= 10;
        }
    }
}

struct EncodedString<'a>(&'a mut String);

impl MetadataSink for EncodedString<'_> {
    fn push_str(&mut self, value: &str) -> Result<()> {
        self.0.push_str(value);
        Ok(())
    }

    fn push_char(&mut self, value: char) -> Result<()> {
        self.0.push(value);
        Ok(())
    }

    fn push_text(&mut self, value: &str) -> Result<()> {
        encode_text_into(value, self.0);
        Ok(())
    }

    fn push_i64(&mut self, value: i64) -> Result<()> {
        let _ = write!(&mut *self.0, "{value}");
        Ok(())
    }
}

fn stream_table_metadata(
    schema: &TableSchema,
    auto_increment: Option<(usize, i64)>,
    encoded: &mut impl MetadataSink,
) -> Result<()> {
    encoded.push_str(SCHEMA_PREFIX)?;
    encoded.push_str(&schema.name)?;
    for column in &schema.columns {
        encoded.push_char('|')?;
        encoded.push_str(&column.name)?;
        encoded.push_char(':')?;
        encoded.push_char(type_tag(column.data_type))?;
        encoded.push_char(':')?;
        encoded.push_char(if column.nullable { '?' } else { '!' })?;
    }
    encoded.push_char(';')?;

    if let Some(primary_key) = schema.primary_key {
        let column = &schema.columns[primary_key];
        encoded.push_str(PRIMARY_KEY_PREFIX)?;
        encoded.push_str(&schema.name)?;
        encoded.push_char('|')?;
        encoded.push_str(&column.name)?;
        encoded.push_char(';')?;
    }

    // Resolved schemas and decoded metadata retain foreign keys in increasing
    // local-column order, so direct iteration preserves canonical encoding.
    for foreign_key in &schema.foreign_keys {
        let column = &schema.columns[foreign_key.column];
        encoded.push_str(FOREIGN_KEY_PREFIX)?;
        encoded.push_str(&schema.name)?;
        encoded.push_char('|')?;
        encoded.push_str(&column.name)?;
        encoded.push_char('|')?;
        encoded.push_str(&foreign_key.referenced_table)?;
        encoded.push_char('|')?;
        encoded.push_str(&foreign_key.referenced_column)?;
        encoded.push_char(';')?;
    }

    if let Some((column, last)) = auto_increment {
        stream_auto_increment_record(schema, column, last, encoded)?;
    }

    for definition in &schema.columns {
        let Some(value) = &definition.default else {
            continue;
        };
        encoded.push_str(DEFAULT_PREFIX)?;
        encoded.push_str(&schema.name)?;
        encoded.push_char('|')?;
        encoded.push_str(&definition.name)?;
        encoded.push_char('|')?;
        stream_typed_value(value, definition.data_type, encoded)?;
        encoded.push_char(';')?;
    }

    for &column in &schema.unique_columns {
        let definition = &schema.columns[column];
        encoded.push_str(UNIQUE_PREFIX)?;
        encoded.push_str(&schema.name)?;
        encoded.push_char('|')?;
        encoded.push_str(&definition.name)?;
        encoded.push_char(';')?;
    }
    Ok(())
}

fn stream_auto_increment_record(
    schema: &TableSchema,
    column: usize,
    last: i64,
    encoded: &mut impl MetadataSink,
) -> Result<()> {
    let definition = &schema.columns[column];
    encoded.push_str(AUTO_INCREMENT_PREFIX)?;
    encoded.push_str(&schema.name)?;
    encoded.push_char('|')?;
    encoded.push_str(&definition.name)?;
    encoded.push_str("|I")?;
    encoded.push_i64(last)?;
    encoded.push_char(';')
}

fn stream_typed_value(
    value: &Value,
    data_type: DataType,
    encoded: &mut impl MetadataSink,
) -> Result<()> {
    match (value, data_type) {
        (Value::Null, _) => encoded.push_char('N'),
        (Value::Text(value), DataType::Text) => {
            encoded.push_char('T')?;
            encoded.push_text(value)
        }
        (Value::Integer(value), DataType::Integer) => {
            encoded.push_char('I')?;
            encoded.push_i64(*value)
        }
        (Value::Boolean(false), DataType::Boolean) => encoded.push_str("B0"),
        (Value::Boolean(true), DataType::Boolean) => encoded.push_str("B1"),
        _ => unreachable!("metadata validation guarantees typed values"),
    }
}
