//! Decoding of schema and constraint metadata records.

use std::collections::BTreeSet;

use super::super::TableSchema;
use super::super::format::{
    AUTO_INCREMENT_PREFIX, FOREIGN_KEY_PREFIX, PRIMARY_KEY_PREFIX, SCHEMA_PREFIX, allocation_error,
    complete_record_body, corrupt, is_valid_identifier,
};
use super::decode_integer;
use crate::{DataType, Result, SchemaColumn};

pub(in crate::storage) struct PrimaryKeyMetadata<'a> {
    pub(in crate::storage) table: &'a str,
    pub(in crate::storage) column: &'a str,
}

pub(in crate::storage) struct ForeignKeyMetadata<'a> {
    pub(in crate::storage) table: &'a str,
    pub(in crate::storage) column: &'a str,
    pub(in crate::storage) referenced_table: &'a str,
    pub(in crate::storage) referenced_column: &'a str,
}

pub(in crate::storage) struct AutoIncrementMetadata<'a> {
    pub(in crate::storage) table: &'a str,
    pub(in crate::storage) column: &'a str,
    pub(in crate::storage) last: i64,
}

pub(in crate::storage) fn decode_schema_record(record: &str, offset: usize) -> Result<TableSchema> {
    let body = complete_record_body(record, SCHEMA_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields
        .next()
        .ok_or_else(|| corrupt(offset, "schema is missing a table name"))?;
    if !is_valid_identifier(table) {
        return Err(corrupt(
            offset + SCHEMA_PREFIX.len(),
            "invalid or noncanonical table name",
        ));
    }

    let mut columns = Vec::new();
    let column_count = body.bytes().filter(|byte| *byte == b'|').count();
    columns
        .try_reserve_exact(column_count)
        .map_err(|_| allocation_error("reserving decoded schema columns"))?;
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
        columns.push(SchemaColumn {
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
        primary_key: None,
        foreign_keys: Vec::new(),
    })
}

pub(in crate::storage) fn decode_primary_key_record(
    record: &str,
    offset: usize,
) -> Result<PrimaryKeyMetadata<'_>> {
    let body = complete_record_body(record, PRIMARY_KEY_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    let column = fields.next().unwrap_or_default();
    if fields.next().is_some() || !is_valid_identifier(table) || !is_valid_identifier(column) {
        return Err(corrupt(offset, "malformed primary-key metadata"));
    }
    Ok(PrimaryKeyMetadata { table, column })
}

pub(in crate::storage) fn decode_foreign_key_record(
    record: &str,
    offset: usize,
) -> Result<ForeignKeyMetadata<'_>> {
    let body = complete_record_body(record, FOREIGN_KEY_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    let column = fields.next().unwrap_or_default();
    let referenced_table = fields.next().unwrap_or_default();
    let referenced_column = fields.next().unwrap_or_default();
    if fields.next().is_some()
        || !is_valid_identifier(table)
        || !is_valid_identifier(column)
        || !is_valid_identifier(referenced_table)
        || !is_valid_identifier(referenced_column)
    {
        return Err(corrupt(offset, "malformed foreign-key metadata"));
    }
    Ok(ForeignKeyMetadata {
        table,
        column,
        referenced_table,
        referenced_column,
    })
}

pub(in crate::storage) fn decode_auto_increment_record(
    record: &str,
    offset: usize,
) -> Result<AutoIncrementMetadata<'_>> {
    let body = complete_record_body(record, AUTO_INCREMENT_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    let column = fields.next().unwrap_or_default();
    let encoded_last = fields.next().unwrap_or_default();
    if fields.next().is_some() || !is_valid_identifier(table) || !is_valid_identifier(column) {
        return Err(corrupt(offset, "malformed auto-increment metadata"));
    }
    let payload = encoded_last
        .strip_prefix('I')
        .ok_or_else(|| corrupt(offset, "auto-increment high-water mark must be an INTEGER"))?;
    let payload_offset = offset + AUTO_INCREMENT_PREFIX.len() + table.len() + 1 + column.len() + 2;
    let last = decode_integer(payload, payload_offset)?;
    Ok(AutoIncrementMetadata {
        table,
        column,
        last,
    })
}
