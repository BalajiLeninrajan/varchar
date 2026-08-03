//! Decoding of schema and constraint metadata records.

use super::super::budget::WorkingStringSet;
use super::super::format::{
    AUTO_INCREMENT_PREFIX, CHECK_PREFIX, DEFAULT_PREFIX, FOREIGN_KEY_PREFIX, PRIMARY_KEY_PREFIX,
    SCHEMA_PREFIX, UNIQUE_PREFIX, complete_record_body, corrupt, is_valid_identifier,
};
use super::super::{ForeignKeyDeleteAction, ForeignKeyUpdateAction, TableSchema};
use super::decode_integer;
use crate::limits::ByteBudget;
use crate::{DataType, Result, SchemaColumn};

const LINEAR_COLUMN_NAME_LIMIT: usize = 4;

pub(in crate::storage) struct PrimaryKeyMetadata<'a> {
    pub(in crate::storage) table: &'a str,
    pub(in crate::storage) column: &'a str,
}

pub(in crate::storage) struct ForeignKeyMetadata<'a> {
    pub(in crate::storage) table: &'a str,
    pub(in crate::storage) column: &'a str,
    pub(in crate::storage) referenced_table: &'a str,
    pub(in crate::storage) referenced_column: &'a str,
    pub(in crate::storage) on_delete: ForeignKeyDeleteAction,
    pub(in crate::storage) on_update: ForeignKeyUpdateAction,
}

pub(in crate::storage) struct AutoIncrementMetadata<'a> {
    pub(in crate::storage) table: &'a str,
    pub(in crate::storage) column: &'a str,
    pub(in crate::storage) last: i64,
}

pub(in crate::storage) struct DefaultMetadata<'a> {
    pub(in crate::storage) table: &'a str,
    pub(in crate::storage) column: &'a str,
    pub(in crate::storage) encoded_value: &'a str,
    pub(in crate::storage) value_offset: usize,
}

pub(in crate::storage) struct UniqueMetadata<'a> {
    pub(in crate::storage) table: &'a str,
    pub(in crate::storage) column: &'a str,
}

pub(in crate::storage) struct CheckMetadata<'a> {
    pub(in crate::storage) table: &'a str,
    pub(in crate::storage) program: &'a str,
    pub(in crate::storage) program_offset: usize,
}

pub(in crate::storage) fn decode_schema_record(
    record: &str,
    offset: usize,
    budget: &mut ByteBudget,
) -> Result<TableSchema> {
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

    let column_count = body.bytes().filter(|byte| *byte == b'|').count();
    let mut columns: Vec<SchemaColumn> = Vec::new();
    budget.reserve_exact(
        &mut columns,
        column_count,
        "reserving decoded schema columns",
    )?;
    let table_name = budget.clone_text(table, "allocating a decoded table name")?;
    let mut column_names = if column_count > LINEAR_COLUMN_NAME_LIMIT {
        Some(WorkingStringSet::new(
            column_count,
            budget,
            "reserving a decoded column-name index",
        )?)
    } else {
        None
    };
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
        let duplicate = if let Some(column_names) = &mut column_names {
            !column_names.insert(name)
        } else {
            columns.iter().any(|column| column.name == name)
        };
        if duplicate {
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
            name: budget.clone_text(name, "allocating a decoded column name")?,
            data_type,
            nullable,
            default: None,
        });
    }
    if columns.is_empty() {
        return Err(corrupt(offset, "table must contain at least one column"));
    }

    Ok(TableSchema {
        name: table_name,
        columns,
        primary_key: None,
        unique_columns: Vec::new(),
        foreign_keys: Vec::new(),
        checks: Vec::new(),
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
    let delete_tag = fields.next();
    let update_tag = fields.next();
    if fields.next().is_some()
        || !is_valid_identifier(table)
        || !is_valid_identifier(column)
        || !is_valid_identifier(referenced_table)
        || !is_valid_identifier(referenced_column)
    {
        return Err(corrupt(offset, "malformed foreign-key metadata"));
    }

    let (on_delete, on_update) = match (delete_tag, update_tag) {
        (None, None) => (
            ForeignKeyDeleteAction::Restrict,
            ForeignKeyUpdateAction::Restrict,
        ),
        (Some(delete_tag), Some(update_tag)) => {
            let on_delete = match delete_tag {
                "R" => ForeignKeyDeleteAction::Restrict,
                "C" => ForeignKeyDeleteAction::Cascade,
                "N" => ForeignKeyDeleteAction::SetNull,
                _ => return Err(corrupt(offset, "malformed foreign-key action metadata")),
            };
            let on_update = match update_tag {
                "R" => ForeignKeyUpdateAction::Restrict,
                "C" => ForeignKeyUpdateAction::Cascade,
                _ => return Err(corrupt(offset, "malformed foreign-key action metadata")),
            };
            if on_delete == ForeignKeyDeleteAction::Restrict
                && on_update == ForeignKeyUpdateAction::Restrict
            {
                return Err(corrupt(
                    offset,
                    "explicit RESTRICT/RESTRICT foreign-key actions are noncanonical",
                ));
            }
            (on_delete, on_update)
        }
        _ => return Err(corrupt(offset, "malformed foreign-key metadata")),
    };

    Ok(ForeignKeyMetadata {
        table,
        column,
        referenced_table,
        referenced_column,
        on_delete,
        on_update,
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

pub(in crate::storage) fn decode_default_record(
    record: &str,
    offset: usize,
) -> Result<DefaultMetadata<'_>> {
    let body = complete_record_body(record, DEFAULT_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    let column = fields.next().unwrap_or_default();
    let encoded_value = fields.next().unwrap_or_default();
    if fields.next().is_some()
        || !is_valid_identifier(table)
        || !is_valid_identifier(column)
        || encoded_value.is_empty()
    {
        return Err(corrupt(offset, "malformed DEFAULT metadata"));
    }
    let value_offset = offset + DEFAULT_PREFIX.len() + table.len() + 1 + column.len() + 1;
    Ok(DefaultMetadata {
        table,
        column,
        encoded_value,
        value_offset,
    })
}

pub(in crate::storage) fn decode_unique_record(
    record: &str,
    offset: usize,
) -> Result<UniqueMetadata<'_>> {
    let body = complete_record_body(record, UNIQUE_PREFIX, offset)?;
    let mut fields = body.split('|');
    let table = fields.next().unwrap_or_default();
    let column = fields.next().unwrap_or_default();
    if fields.next().is_some() || !is_valid_identifier(table) || !is_valid_identifier(column) {
        return Err(corrupt(offset, "malformed UNIQUE metadata"));
    }
    Ok(UniqueMetadata { table, column })
}

pub(in crate::storage) fn decode_check_record(
    record: &str,
    offset: usize,
) -> Result<CheckMetadata<'_>> {
    let body = complete_record_body(record, CHECK_PREFIX, offset)?;
    let (table, program) = body.split_once('|').ok_or_else(|| {
        corrupt(
            offset + record.len() - 1,
            "CHECK metadata is missing its program",
        )
    })?;
    if !is_valid_identifier(table) {
        return Err(corrupt(
            offset + CHECK_PREFIX.len(),
            "invalid or noncanonical CHECK table name",
        ));
    }
    let program_offset = offset + CHECK_PREFIX.len() + table.len() + 1;
    if program.is_empty() {
        return Err(corrupt(
            program_offset,
            "CHECK metadata is missing its program",
        ));
    }
    Ok(CheckMetadata {
        table,
        program,
        program_offset,
    })
}
