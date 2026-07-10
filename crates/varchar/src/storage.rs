//! Canonical encoding for the single-string database.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::{Column, DataType, Error, Result, Value};

/// The canonical empty database.
pub(crate) const EMPTY_BLOB: &str = "V1;";

/// The disposable schema index reconstructed from the authoritative string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Catalog {
    pub(crate) tables: BTreeMap<String, TableSchema>,
    /// Byte offset at which another schema record can be inserted.
    pub(crate) row_start: usize,
}

impl Catalog {
    pub(crate) fn table(&self, name: &str) -> Option<&TableSchema> {
        self.tables.get(name)
    }
}

/// A table definition reconstructed from a schema record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TableSchema {
    pub(crate) name: String,
    pub(crate) columns: Vec<Column>,
}

/// Validate an entire blob and reconstruct its disposable schema catalog.
pub(crate) fn validate_and_catalog(blob: &str) -> Result<Catalog> {
    if !blob.starts_with(EMPTY_BLOB) {
        return Err(corrupt(0, "expected canonical V1; header"));
    }

    let mut tables = BTreeMap::new();
    let mut offset = EMPTY_BLOB.len();
    let mut row_start = blob.len();
    let mut saw_row = false;

    while offset < blob.len() {
        if !blob[offset..].starts_with('~') {
            return Err(corrupt(offset, "expected a schema or row record"));
        }
        let relative_end = blob[offset..]
            .find(';')
            .ok_or_else(|| corrupt(offset, "unterminated record"))?;
        let end = offset + relative_end + 1;
        let record = &blob[offset..end];

        if record.starts_with("~S|") {
            if saw_row {
                return Err(corrupt(offset, "schema record appears after a row record"));
            }
            let schema = decode_schema_record(record, offset)?;
            if tables.contains_key(&schema.name) {
                return Err(corrupt(offset, "duplicate table schema"));
            }
            tables.insert(schema.name.clone(), schema);
        } else if record.starts_with("~R|") {
            if !saw_row {
                row_start = offset;
                saw_row = true;
            }
            validate_row_record(record, offset, &tables)?;
        } else {
            return Err(corrupt(offset, "unknown record tag"));
        }

        offset = end;
    }

    Ok(Catalog { tables, row_start })
}

/// Encode a complete schema record, including its terminator.
pub(crate) fn encode_schema(schema: &TableSchema) -> Result<String> {
    validate_schema_for_write(schema)?;

    let mut encoded = String::from("~S|");
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

/// Decode a complete canonical row record for `schema`.
pub(crate) fn decode_row(record: &str, schema: &TableSchema) -> Result<Vec<Value>> {
    decode_row_at(record, schema, 0)
}

/// Identifiers in storage are already canonicalized lowercase ASCII.
pub(crate) fn is_valid_identifier(identifier: &str) -> bool {
    let mut bytes = identifier.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first == b'_' || first.is_ascii_lowercase())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn decode_schema_record(record: &str, offset: usize) -> Result<TableSchema> {
    let body = complete_record_body(record, "~S|", offset)?;
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

fn validate_row_record(
    record: &str,
    offset: usize,
    tables: &BTreeMap<String, TableSchema>,
) -> Result<()> {
    let body = complete_record_body(record, "~R|", offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    if !is_valid_identifier(table) {
        return Err(corrupt(offset + 3, "invalid or noncanonical table name"));
    }
    let schema = tables
        .get(table)
        .ok_or_else(|| corrupt(offset, "row references an unknown table"))?;

    let mut cell_offset = offset + "~R|".len() + table.len() + 1;
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
    let body = complete_record_body(record, "~R|", offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    if table != schema.name {
        return Err(corrupt(offset, "row table does not match its schema"));
    }

    let mut values = Vec::new();
    values
        .try_reserve_exact(schema.columns.len())
        .map_err(|_| allocation_limit("decoded row cells", schema.columns.len()))?;
    let mut cell_offset = offset + "~R|".len() + table.len() + 1;
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

fn complete_record_body<'a>(record: &'a str, prefix: &str, offset: usize) -> Result<&'a str> {
    let body = record
        .strip_prefix(prefix)
        .ok_or_else(|| corrupt(offset, "unexpected record tag"))?
        .strip_suffix(';')
        .ok_or_else(|| corrupt(offset, "unterminated record"))?;
    if body.contains(';') {
        return Err(corrupt(offset, "raw semicolon inside record"));
    }
    Ok(body)
}

fn validate_schema_for_write(schema: &TableSchema) -> Result<()> {
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
    let mut names = BTreeSet::new();
    for column in &schema.columns {
        if !is_valid_identifier(&column.name) {
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

fn encode_text_into(value: &str, encoded: &mut String) {
    for character in value.chars() {
        if must_escape(character) {
            encoded.push('%');
            // Writing to a String is infallible.
            let _ = write!(encoded, "{:06X}", character as u32);
        } else {
            encoded.push(character);
        }
    }
}

fn decode_text(payload: &str, offset: usize) -> Result<String> {
    let mut decoded = String::new();
    decoded
        .try_reserve(payload.len())
        .map_err(|_| allocation_limit("decoded text bytes", payload.len()))?;
    scan_text(payload, offset, |character| decoded.push(character))?;
    Ok(decoded)
}

fn scan_text(payload: &str, offset: usize, mut accept: impl FnMut(char)) -> Result<()> {
    let bytes = payload.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 7 > bytes.len() {
                return Err(corrupt(offset + index, "truncated text escape"));
            }
            let digits = &bytes[index + 1..index + 7];
            if !digits
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(byte))
            {
                return Err(corrupt(offset + index, "malformed text escape"));
            }
            let digits = std::str::from_utf8(digits)
                .map_err(|_| corrupt(offset + index, "malformed text escape"))?;
            let scalar = u32::from_str_radix(digits, 16)
                .map_err(|_| corrupt(offset + index, "malformed text escape"))?;
            let character = char::from_u32(scalar)
                .ok_or_else(|| corrupt(offset + index, "escape is not a Unicode scalar"))?;
            if !must_escape(character) {
                return Err(corrupt(
                    offset + index,
                    "unnecessary noncanonical text escape",
                ));
            }
            accept(character);
            index += 7;
            continue;
        }

        let character = payload[index..]
            .chars()
            .next()
            .ok_or_else(|| corrupt(offset + index, "invalid UTF-8 text payload"))?;
        if must_escape(character) {
            return Err(corrupt(offset + index, "unescaped structural character"));
        }
        accept(character);
        index += character.len_utf8();
    }
    Ok(())
}

fn must_escape(character: char) -> bool {
    matches!(character, '%' | '~' | '|' | ';' | '\u{2028}' | '\u{2029}') || character.is_control()
}

fn type_tag(data_type: DataType) -> char {
    match data_type {
        DataType::Text => 'T',
        DataType::Integer => 'I',
        DataType::Boolean => 'B',
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

fn corrupt(offset: usize, message: impl Into<String>) -> Error {
    Error::CorruptStorage {
        offset,
        message: message.into(),
    }
}

fn allocation_limit(resource: &'static str, attempted: usize) -> Error {
    Error::ResourceLimit {
        resource,
        limit: attempted,
    }
}
