//! Cross-row primary-key and foreign-key integrity validation.

use std::collections::{BTreeMap, BTreeSet};

use super::decode::row_records;
use super::{Catalog, TableSchema};
use crate::Error;

pub(super) enum ValidationError {
    Storage(Error),
    Constraint(Violation),
}

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

impl From<Violation> for ValidationError {
    fn from(violation: Violation) -> Self {
        Self::Constraint(violation)
    }
}

type ConstraintResult<T> = std::result::Result<T, Violation>;
type ValidationResult<T> = std::result::Result<T, ValidationError>;

pub(super) fn validate_rows<'a>(blob: &'a str, catalog: &Catalog) -> ValidationResult<()> {
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

    for_each_row(blob, catalog, |row, schema| {
        let offset = row.range().start;
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

    for_each_row(blob, catalog, |row, schema| {
        let offset = row.range().start;
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
    mut visit: impl FnMut(&super::decode::RowRecordRef<'a>, &TableSchema) -> ConstraintResult<()>,
) -> ValidationResult<()> {
    for row in row_records(blob, catalog.row_start) {
        let row = row.map_err(ValidationError::Storage)?;
        let row_range = row.range();
        let schema = catalog.tables.get(row.table()).ok_or_else(|| {
            Violation::new(
                row_range.start,
                "row table disappeared during integrity validation",
            )
        })?;
        visit(&row, schema)?;
    }
    Ok(())
}
