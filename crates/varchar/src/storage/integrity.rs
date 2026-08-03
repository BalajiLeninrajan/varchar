//! Cross-row primary-key and foreign-key integrity validation.

use std::cmp::Ordering;

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

struct PrimaryValues<'a> {
    table: &'a str,
    values: PrimaryValueSet<'a>,
}

enum PrimaryValueSet<'a> {
    Single(Option<&'a str>),
    Multiple(Vec<&'a str>),
}

impl<'a> PrimaryValueSet<'a> {
    /// Records a key as the fill pass sees it, promoting a lone key to a grown index.
    fn push(&mut self, value: &'a str) {
        match self {
            Self::Single(slot @ None) => {
                *slot = Some(value);
            }
            Self::Single(slot) => {
                let existing = slot.expect("the matched slot holds a key");
                *self = Self::Multiple(vec![existing, value]);
            }
            Self::Multiple(values) => values.push(value),
        }
    }

    fn duplicate_occurrence(&mut self) -> Option<&'a str> {
        let Self::Multiple(values) = self else {
            return None;
        };
        values.sort_unstable_by(|left, right| {
            compare_primary_values(left, right)
                .then_with(|| source_position(left).cmp(&source_position(right)))
        });
        values
            .windows(2)
            .filter(|pair| pair[0] == pair[1])
            .map(|pair| pair[1])
            .min_by_key(|value| source_position(value))
    }

    fn contains(&self, value: &str) -> bool {
        match self {
            Self::Single(existing) => existing.is_some_and(|existing| existing == value),
            Self::Multiple(values) => values
                .binary_search_by(|existing| lookup_primary_value(existing, value))
                .is_ok(),
        }
    }
}

fn source_position(value: &str) -> usize {
    value.as_ptr() as usize
}

fn compare_primary_values(left: &str, right: &str) -> Ordering {
    #[cfg(test)]
    record_working_string_insert_comparison();
    left.cmp(right)
}

fn lookup_primary_value(existing: &str, wanted: &str) -> Ordering {
    #[cfg(test)]
    record_working_string_lookup_comparison();
    existing.cmp(wanted)
}

fn record_earliest(earliest: &mut Option<Violation>, violation: Violation) {
    if earliest
        .as_ref()
        .is_none_or(|existing| violation.offset < existing.offset)
    {
        *earliest = Some(violation);
    }
}

fn primary_values_index(values: &[PrimaryValues<'_>], table: &str) -> Option<usize> {
    values
        .binary_search_by(|values| values.table.cmp(table))
        .ok()
}

pub(super) fn validate_rows(blob: &str, catalog: &Catalog) -> ValidationResult<()> {
    let primary_count = catalog
        .schemas()
        .filter(|schema| schema.primary_key.is_some())
        .count();
    let has_foreign_keys = catalog
        .schemas()
        .any(|schema| !schema.foreign_keys.is_empty());
    if primary_count == 0 {
        debug_assert!(!has_foreign_keys, "every foreign key targets a primary key");
        return Ok(());
    }

    let mut primary_values = Vec::with_capacity(primary_count);
    for (table, schema) in catalog.tables() {
        if schema.primary_key.is_some() {
            primary_values.push(PrimaryValues {
                table,
                values: PrimaryValueSet::Single(None),
            });
        }
    }
    primary_values.sort_unstable_by(|left, right| left.table.cmp(right.table));

    // The fill pass is spelled out rather than delegated to `for_each_row` because a duplicate
    // key is only located once the whole index has been sorted, so the earliest row-order
    // violation has to be remembered rather than returned as it is seen.
    let mut earliest_primary_violation = None;
    for row in row_records(blob, catalog.row_start) {
        let row = row.map_err(ValidationError::Storage)?;
        let offset = row.range().start;
        let schema = catalog.table(row.table()).ok_or_else(|| {
            Violation::new(offset, "row table disappeared during integrity validation")
        })?;
        let Some(primary_key) = schema.primary_key else {
            continue;
        };
        let Some(value) = row.cells().nth(primary_key) else {
            record_earliest(
                &mut earliest_primary_violation,
                Violation::new(offset, "primary-key cell is missing from a validated row"),
            );
            continue;
        };
        if value == "N" {
            record_earliest(
                &mut earliest_primary_violation,
                Violation::new(
                    offset,
                    format!("primary key for table {:?} is NULL", schema.name),
                ),
            );
            continue;
        }
        if let Some(auto_increment) = catalog.auto_increment_state(&schema.name) {
            debug_assert_eq!(auto_increment.column, primary_key);
            let Some(stored) = value
                .strip_prefix('I')
                .and_then(|payload| payload.parse::<i64>().ok())
            else {
                record_earliest(
                    &mut earliest_primary_violation,
                    Violation::new(offset, "auto-increment primary-key cell is not an INTEGER"),
                );
                continue;
            };
            if stored > auto_increment.last {
                record_earliest(
                    &mut earliest_primary_violation,
                    Violation::new(
                        offset,
                        format!(
                            "auto-increment high-water mark for table {:?} is below a stored key",
                            schema.name
                        ),
                    ),
                );
                continue;
            }
        }
        let index = primary_values_index(&primary_values, &schema.name)
            .expect("a primary-key index exists for every keyed table");
        primary_values[index].values.push(value);
    }
    let mut earliest_duplicate = None;
    for values in &mut primary_values {
        let Some(occurrence) = values.values.duplicate_occurrence() else {
            continue;
        };
        if earliest_duplicate.is_none_or(|(_, existing): (&str, &str)| {
            source_position(occurrence) < source_position(existing)
        }) {
            earliest_duplicate = Some((values.table, occurrence));
        }
    }
    if let Some((table, occurrence)) = earliest_duplicate {
        for row in row_records(blob, catalog.row_start) {
            let row = row.map_err(ValidationError::Storage)?;
            if row.table() != table {
                continue;
            }
            let schema = catalog
                .table(table)
                .expect("a primary-key index names a catalog table");
            let primary_key = schema
                .primary_key
                .expect("a primary-key index names a keyed table");
            let value = row
                .cells()
                .nth(primary_key)
                .expect("validated rows contain their primary-key cell");
            if value.len() == occurrence.len() && std::ptr::eq(value.as_ptr(), occurrence.as_ptr())
            {
                record_earliest(
                    &mut earliest_primary_violation,
                    Violation::new(
                        row.range().start,
                        format!("duplicate primary key in table {table:?}"),
                    ),
                );
                break;
            }
        }
    }
    if let Some(violation) = earliest_primary_violation {
        return Err(violation.into());
    }

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
            let referenced_values = &primary_values[primary_values_index(
                &primary_values,
                &foreign_key.referenced_table,
            )
            .expect("every foreign key was resolved to a primary key")];
            if !referenced_values.values.contains(value) {
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
        let schema = catalog.table(row.table()).ok_or_else(|| {
            Violation::new(
                row_range.start,
                "row table disappeared during integrity validation",
            )
        })?;
        visit(&row, schema)?;
    }
    Ok(())
}

#[cfg(test)]
std::thread_local! {
    static WORKING_STRING_COMPARISONS: std::cell::Cell<(usize, usize)> = const {
        std::cell::Cell::new((0, 0))
    };
}

#[cfg(test)]
pub(super) fn record_working_string_insert_comparison() {
    WORKING_STRING_COMPARISONS.with(|comparisons| {
        let (insert, lookup) = comparisons.get();
        comparisons.set((insert + 1, lookup));
    });
}

#[cfg(test)]
pub(super) fn record_working_string_lookup_comparison() {
    WORKING_STRING_COMPARISONS.with(|comparisons| {
        let (insert, lookup) = comparisons.get();
        comparisons.set((insert, lookup + 1));
    });
}

#[cfg(test)]
pub(super) fn reset_working_string_comparisons() {
    WORKING_STRING_COMPARISONS.with(|comparisons| comparisons.set((0, 0)));
}

#[cfg(test)]
pub(super) fn working_string_comparisons() -> (usize, usize) {
    WORKING_STRING_COMPARISONS.with(std::cell::Cell::get)
}
