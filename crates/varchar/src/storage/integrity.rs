//! Cross-row primary-key and foreign-key integrity validation.

use std::collections::{BTreeMap, BTreeSet};

use super::format::{RecordKind, records};
use super::{Catalog, RowRecordRef, TableSchema, row_record};
use crate::Error;

pub(super) struct Violation {
    pub(super) offset: usize,
    pub(super) message: String,
}

impl Violation {
    fn new(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset,
            message: message.into(),
        }
    }
}

type IntegrityResult<T> = std::result::Result<T, Violation>;

pub(super) fn validate_rows<'a>(blob: &'a str, catalog: &Catalog) -> IntegrityResult<()> {
    let has_primary_keys = catalog
        .tables
        .values()
        .any(|schema| schema.primary_key.is_some());
    let has_foreign_keys = catalog
        .tables
        .values()
        .any(|schema| !schema.foreign_keys.is_empty());
    if !has_primary_keys {
        debug_assert!(!has_foreign_keys, "every foreign key targets a primary key");
        return Ok(());
    }

    let mut primary_values = catalog
        .tables
        .iter()
        .filter_map(|(table, schema)| {
            schema
                .primary_key
                .map(|_| (table.as_str(), BTreeSet::<&'a str>::new()))
        })
        .collect::<BTreeMap<_, _>>();

    for_each_row(blob, catalog, |row, schema, offset| {
        let Some(primary_key) = schema.primary_key else {
            return Ok(());
        };
        let value = row.cells().nth(primary_key).ok_or_else(|| {
            Violation::new(offset, "primary-key cell is missing from a validated row")
        })?;
        if value == "N" {
            return Err(Violation::new(
                offset,
                format!("primary key for table {:?} is NULL", schema.name),
            ));
        }
        if let Some(auto_increment) = catalog.auto_increment_state(&schema.name) {
            debug_assert_eq!(auto_increment.column, primary_key);
            let stored = value
                .strip_prefix('I')
                .and_then(|payload| payload.parse::<i64>().ok())
                .ok_or_else(|| {
                    Violation::new(offset, "auto-increment primary-key cell is not an INTEGER")
                })?;
            if stored > auto_increment.last {
                return Err(Violation::new(
                    offset,
                    format!(
                        "auto-increment high-water mark for table {:?} is below a stored key",
                        schema.name
                    ),
                ));
            }
        }
        let values = primary_values
            .get_mut(schema.name.as_str())
            .expect("a primary-key set exists for every keyed table");
        if !values.insert(value) {
            return Err(Violation::new(
                offset,
                format!("duplicate primary key in table {:?}", schema.name),
            ));
        }
        Ok(())
    })?;

    if !has_foreign_keys {
        return Ok(());
    }

    for_each_row(blob, catalog, |row, schema, offset| {
        if schema.foreign_keys.is_empty() {
            return Ok(());
        }
        let mut foreign_keys = schema.foreign_keys.iter().peekable();
        for (column, value) in row.cells().enumerate() {
            let Some(foreign_key) = foreign_keys.peek() else {
                break;
            };
            if foreign_key.column != column {
                continue;
            }
            let foreign_key = foreign_keys
                .next()
                .expect("the peeked foreign key is present");
            if value == "N" {
                continue;
            }
            let referenced_values = primary_values
                .get(foreign_key.referenced_table.as_str())
                .expect("every foreign key was resolved to a primary key");
            if !referenced_values.contains(value) {
                return Err(Violation::new(
                    offset,
                    format!(
                        "foreign key {:?}.{:?} has no matching row in {:?}.{:?}",
                        schema.name,
                        schema.columns[foreign_key.column].name,
                        foreign_key.referenced_table,
                        foreign_key.referenced_column
                    ),
                ));
            }
        }
        if foreign_keys.next().is_some() {
            return Err(Violation::new(
                offset,
                "foreign-key cell is missing from a validated row",
            ));
        }
        Ok(())
    })
}

fn for_each_row<'a>(
    blob: &'a str,
    catalog: &Catalog,
    mut visit: impl FnMut(&RowRecordRef<'a>, &TableSchema, usize) -> IntegrityResult<()>,
) -> IntegrityResult<()> {
    for record in records(blob) {
        let record = record.map_err(map_storage_error)?;
        if record.range.start < catalog.row_start {
            continue;
        }
        if record.kind != RecordKind::Row {
            return Err(Violation::new(
                record.range.start,
                "non-row record during integrity validation",
            ));
        }
        let row = row_record(record.text, record.range.start).map_err(map_storage_error)?;
        let row_range = row.range();
        let schema = catalog.tables.get(row.table()).ok_or_else(|| {
            Violation::new(
                row_range.start,
                "row table disappeared during integrity validation",
            )
        })?;
        visit(&row, schema, row_range.start)?;
    }
    Ok(())
}

fn map_storage_error(error: Error) -> Violation {
    match error {
        Error::CorruptStorage { offset, message } => Violation::new(offset, message),
        error => Violation::new(0, error.to_string()),
    }
}
