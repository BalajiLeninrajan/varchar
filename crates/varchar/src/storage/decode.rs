use std::collections::{BTreeMap, BTreeSet};

use super::TableSchema;
use super::format::{
    ROW_PREFIX, SCHEMA_PREFIX, allocation_limit, complete_record_body, corrupt,
    is_valid_identifier, scan_text,
};
use crate::{Column, DataType, Error, Result, Value};

/// Decode a complete canonical row record for `schema`.
pub(crate) fn decode_row(record: &str, schema: &TableSchema) -> Result<Vec<Value>> {
    decode_row_at(record, schema, 0)
}

pub(super) fn decode_schema_record(record: &str, offset: usize) -> Result<TableSchema> {
    let body = complete_record_body(record, SCHEMA_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields
        .next()
        .ok_or_else(|| corrupt(offset, "schema is missing a table name"))?;
    if !is_valid_identifier(table) {
        return Err(corrupt(offset + 3, "invalid or noncanonical table name"));
    }

    let mut columns = Vec::new();
    let column_count = body.bytes().filter(|byte| *byte == b'|').count();
    columns
        .try_reserve_exact(column_count)
        .map_err(|_| allocation_limit("schema columns", column_count))?;
    let mut names = BTreeSet::new();
    for field in fields {
        let mut parts = field.split(':');
        let name = parts.next().unwrap_or_default();
        let data_type = parts.next();
        let nullability = parts.next();
        if parts.next().is_some() || data_type.is_none() || nullability.is_none() {
            return Err(corrupt(offset, "malformed column descriptor"));
        }
        if !is_valid_identifier(name) {
            return Err(corrupt(offset, "invalid or noncanonical column name"));
        }
        if !names.insert(name) {
            return Err(corrupt(offset, "duplicate column name"));
        }
        let data_type = match data_type.unwrap() {
            "T" => DataType::Text,
            "I" => DataType::Integer,
            "B" => DataType::Boolean,
            _ => return Err(corrupt(offset, "unknown column type tag")),
        };
        let nullable = match nullability.unwrap() {
            "?" => true,
            "!" => false,
            _ => return Err(corrupt(offset, "invalid column nullability tag")),
        };
        columns.push(Column {
            name: name.to_owned(),
            data_type,
            nullable,
        });
    }
    if columns.is_empty() {
        return Err(corrupt(offset, "table must contain at least one column"));
    }

    Ok(TableSchema {
        name: table.to_owned(),
        columns,
    })
}

pub(super) fn validate_row_record(
    record: &str,
    offset: usize,
    tables: &BTreeMap<String, TableSchema>,
) -> Result<()> {
    let body = complete_record_body(record, ROW_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    if !is_valid_identifier(table) {
        return Err(corrupt(offset + 3, "invalid or noncanonical table name"));
    }
    let schema = tables
        .get(table)
        .ok_or_else(|| corrupt(offset, "row references an unknown table"))?;

    let mut cell_offset = offset + ROW_PREFIX.len() + table.len() + 1;
    let mut cell_count = 0;
    for column in &schema.columns {
        let Some(cell) = fields.next() else {
            return Err(row_width_error(offset, schema, cell_count));
        };
        validate_cell_at(cell, column, cell_offset)?;
        cell_count += 1;
        cell_offset += cell.len() + 1;
    }
    if fields.next().is_some() {
        cell_count += 1 + fields.count();
        return Err(row_width_error(offset, schema, cell_count));
    }
    Ok(())
}

fn decode_row_at(record: &str, schema: &TableSchema, offset: usize) -> Result<Vec<Value>> {
    let body = complete_record_body(record, ROW_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    if table != schema.name {
        return Err(corrupt(offset, "row table does not match its schema"));
    }

    let mut values = Vec::new();
    values
        .try_reserve_exact(schema.columns.len())
        .map_err(|_| allocation_limit("decoded row cells", schema.columns.len()))?;
    let mut cell_offset = offset + ROW_PREFIX.len() + table.len() + 1;
    for column in &schema.columns {
        let Some(cell) = fields.next() else {
            return Err(row_width_error(offset, schema, values.len()));
        };
        values.push(decode_cell_at(cell, column, cell_offset)?);
        cell_offset += cell.len() + 1;
    }
    if fields.next().is_some() {
        let cell_count = values.len() + 1 + fields.count();
        return Err(row_width_error(offset, schema, cell_count));
    }
    Ok(values)
}

fn row_width_error(offset: usize, schema: &TableSchema, actual: usize) -> Error {
    corrupt(
        offset,
        format!(
            "row for {:?} has {} cells, expected {}",
            schema.name,
            actual,
            schema.columns.len()
        ),
    )
}

fn validate_cell_at(encoded: &str, column: &Column, offset: usize) -> Result<()> {
    if encoded == "N" {
        return if column.nullable {
            Ok(())
        } else {
            Err(corrupt(offset, "NULL stored in a NOT NULL column"))
        };
    }

    match column.data_type {
        DataType::Text => {
            let payload = encoded
                .strip_prefix('T')
                .ok_or_else(|| corrupt(offset, "cell type does not match TEXT column"))?;
            scan_text(payload, offset + 1, |_| {})
        }
        DataType::Integer => {
            let payload = encoded
                .strip_prefix('I')
                .ok_or_else(|| corrupt(offset, "cell type does not match INTEGER column"))?;
            decode_integer(payload, offset + 1).map(|_| ())
        }
        DataType::Boolean => match encoded {
            "B0" | "B1" => Ok(()),
            _ => Err(corrupt(offset, "invalid BOOLEAN cell")),
        },
    }
}

fn decode_cell_at(encoded: &str, column: &Column, offset: usize) -> Result<Value> {
    if encoded == "N" {
        return if column.nullable {
            Ok(Value::Null)
        } else {
            Err(corrupt(offset, "NULL stored in a NOT NULL column"))
        };
    }

    match column.data_type {
        DataType::Text => {
            let payload = encoded
                .strip_prefix('T')
                .ok_or_else(|| corrupt(offset, "cell type does not match TEXT column"))?;
            decode_text(payload, offset + 1).map(Value::Text)
        }
        DataType::Integer => {
            let payload = encoded
                .strip_prefix('I')
                .ok_or_else(|| corrupt(offset, "cell type does not match INTEGER column"))?;
            decode_integer(payload, offset + 1).map(Value::Integer)
        }
        DataType::Boolean => match encoded {
            "B0" => Ok(Value::Boolean(false)),
            "B1" => Ok(Value::Boolean(true)),
            _ => Err(corrupt(offset, "invalid BOOLEAN cell")),
        },
    }
}

fn decode_integer(payload: &str, offset: usize) -> Result<i64> {
    let value: i64 = payload
        .parse()
        .map_err(|_| corrupt(offset, "invalid INTEGER cell"))?;
    if !is_canonical_integer(payload) {
        return Err(corrupt(offset, "noncanonical INTEGER cell"));
    }
    Ok(value)
}

fn is_canonical_integer(payload: &str) -> bool {
    if payload == "0" {
        return true;
    }
    let digits = payload.strip_prefix('-').unwrap_or(payload);
    let mut bytes = digits.bytes();
    bytes
        .next()
        .is_some_and(|byte| (b'1'..=b'9').contains(&byte))
        && bytes.all(|byte| byte.is_ascii_digit())
}

fn decode_text(payload: &str, offset: usize) -> Result<String> {
    let mut decoded = String::new();
    decoded
        .try_reserve(payload.len())
        .map_err(|_| allocation_limit("decoded text bytes", payload.len()))?;
    scan_text(payload, offset, |character| decoded.push(character))?;
    Ok(decoded)
}
