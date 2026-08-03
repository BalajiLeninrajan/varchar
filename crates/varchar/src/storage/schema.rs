//! Table schemas and the physical row layouts they define.

use super::format;
use crate::{Error, Result, SchemaColumn};

/// The physical shape required to encode, decode, or scan one table's rows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowLayout<'a> {
    pub(crate) table: &'a str,
    pub(crate) columns: &'a [SchemaColumn],
}

/// Proof that a physical row layout passed canonical schema validation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ValidatedRowLayout<'a> {
    layout: RowLayout<'a>,
}

impl<'a> ValidatedRowLayout<'a> {
    pub(crate) const fn column_count(self) -> usize {
        self.layout.columns.len()
    }

    pub(super) const fn layout(self) -> RowLayout<'a> {
        self.layout
    }
}

/// A table schema borrowed from the validated storage catalog.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ValidatedTableSchema<'a> {
    schema: &'a TableSchema,
}

impl<'a> ValidatedTableSchema<'a> {
    pub(in crate::storage) const fn from_catalog(schema: &'a TableSchema) -> Self {
        Self { schema }
    }

    pub(crate) const fn schema(self) -> &'a TableSchema {
        self.schema
    }

    pub(crate) fn try_clone_row_layout(self) -> Result<OwnedValidatedRowLayout> {
        const OPERATION: &str = "cloning a validated row layout";

        let table = try_clone_string(&self.schema.name, OPERATION)?;
        let mut columns = Vec::new();
        columns
            .try_reserve_exact(self.schema.columns.len())
            .map_err(|_| format::allocation_error(OPERATION))?;
        for column in &self.schema.columns {
            columns.push(SchemaColumn {
                name: try_clone_string(&column.name, OPERATION)?,
                data_type: column.data_type,
                nullable: column.nullable,
                default: None,
            });
        }
        Ok(OwnedValidatedRowLayout { table, columns })
    }
}

fn try_clone_string(value: &str, operation: &'static str) -> Result<String> {
    let mut cloned = String::new();
    cloned
        .try_reserve_exact(value.len())
        .map_err(|_| format::allocation_error(operation))?;
    cloned.push_str(value);
    Ok(cloned)
}

/// An owned clone of a catalog-validated physical row layout.
#[derive(Debug)]
pub(crate) struct OwnedValidatedRowLayout {
    table: String,
    columns: Vec<SchemaColumn>,
}

impl OwnedValidatedRowLayout {
    pub(crate) fn row_layout(&self) -> RowLayout<'_> {
        RowLayout {
            table: &self.table,
            columns: &self.columns,
        }
    }

    pub(crate) fn validated_row_layout(&self) -> ValidatedRowLayout<'_> {
        ValidatedRowLayout {
            layout: self.row_layout(),
        }
    }
}

/// A table definition reconstructed from its schema and key metadata records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TableSchema {
    pub(crate) name: String,
    pub(crate) columns: Vec<SchemaColumn>,
    pub(crate) primary_key: Option<usize>,
    /// Non-primary single-column UNIQUE constraints in column order.
    pub(crate) unique_columns: Vec<usize>,
    /// Increasing by local column; each local column appears at most once.
    pub(crate) foreign_keys: Vec<ForeignKey>,
    /// Resolved CHECK expressions in declaration order.
    pub(crate) checks: Vec<crate::expression::CheckProgram>,
}

impl TableSchema {
    pub(crate) fn row_layout(&self) -> RowLayout<'_> {
        RowLayout {
            table: &self.name,
            columns: &self.columns,
        }
    }
}

/// A single-column foreign key reconstructed from schema metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ForeignKey {
    /// Index of the referencing column in the local table.
    pub(crate) column: usize,
    pub(crate) referenced_table: String,
    pub(crate) referenced_column: String,
}

pub(crate) fn validate_row_layout<'layout>(
    layout: RowLayout<'layout>,
) -> Result<ValidatedRowLayout<'layout>> {
    #[cfg(test)]
    ROW_LAYOUT_VALIDATIONS.with(|validations| validations.set(validations.get() + 1));

    if !format::is_valid_identifier(layout.table) {
        return Err(Error::Schema(format!(
            "invalid or noncanonical table name {:?}",
            layout.table
        )));
    }
    if layout.columns.is_empty() {
        return Err(Error::Schema(String::from(
            "table must contain at least one column",
        )));
    }
    // Replacement encoding invokes this path under the storage-working budget,
    // so inspect the borrowed prefix instead of allocating a temporary set.
    for (position, column) in layout.columns.iter().enumerate() {
        if !format::is_valid_identifier(&column.name) {
            return Err(Error::Schema(format!(
                "invalid or noncanonical column name {:?}",
                column.name
            )));
        }
        if layout.columns[..position]
            .iter()
            .any(|seen| seen.name == column.name)
        {
            return Err(Error::Schema(format!(
                "duplicate column name {:?}",
                column.name
            )));
        }
    }
    Ok(ValidatedRowLayout { layout })
}

#[cfg(test)]
std::thread_local! {
    static ROW_LAYOUT_VALIDATIONS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
}

#[cfg(test)]
pub(crate) fn reset_row_layout_validations() {
    ROW_LAYOUT_VALIDATIONS.with(|validations| validations.set(0));
}

#[cfg(test)]
pub(crate) fn row_layout_validations() -> usize {
    ROW_LAYOUT_VALIDATIONS.with(std::cell::Cell::get)
}
