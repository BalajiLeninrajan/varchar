//! Cross-row primary-key and foreign-key integrity validation.

use std::collections::{BTreeMap, BTreeSet};

use super::format::ROW_PREFIX;
use super::{Catalog, TableSchema};

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

    for_each_row(blob, catalog, |record, schema, offset| {
        let Some(primary_key) = schema.primary_key else {
            return Ok(());
        };
        let value = row_cells(record)
            .and_then(|mut cells| cells.nth(primary_key))
            .ok_or_else(|| {
                Violation::new(offset, "primary-key cell is missing from a validated row")
            })?;
        if value == "N" {
            return Err(Violation::new(
                offset,
                format!("primary key for table {:?} is NULL", schema.name),
            ));
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

    for_each_row(blob, catalog, |record, schema, offset| {
        if schema.foreign_keys.is_empty() {
            return Ok(());
        }
        let mut foreign_keys = schema.foreign_keys.iter().peekable();
        let cells = row_cells(record)
            .ok_or_else(|| Violation::new(offset, "validated row has no cell list"))?;
        for (column, value) in cells.enumerate() {
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
    mut visit: impl FnMut(&'a str, &TableSchema, usize) -> IntegrityResult<()>,
) -> IntegrityResult<()> {
    let mut offset = catalog.row_start;
    while offset < blob.len() {
        let relative_end = blob[offset..].find(';').ok_or_else(|| {
            Violation::new(offset, "unterminated row during integrity validation")
        })?;
        let end = offset + relative_end + 1;
        let record = &blob[offset..end];
        let body = record
            .strip_prefix(ROW_PREFIX)
            .ok_or_else(|| Violation::new(offset, "non-row record during integrity validation"))?;
        let table = body.split('|').next().unwrap_or_default();
        let schema = catalog.tables.get(table).ok_or_else(|| {
            Violation::new(offset, "row table disappeared during integrity validation")
        })?;
        visit(record, schema, offset)?;
        offset = end;
    }
    Ok(())
}

fn row_cells(record: &str) -> Option<std::str::Split<'_, char>> {
    let body = record.strip_prefix(ROW_PREFIX)?.strip_suffix(';')?;
    let (_, cells) = body.split_once('|')?;
    Some(cells.split('|'))
}
