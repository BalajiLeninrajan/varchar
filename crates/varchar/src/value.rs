//! SQL values and engine-produced result snapshots.

use std::fmt;

use crate::{Error, Result};

/// A column's SQL type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataType {
    /// A UTF-8 text value.
    Text,
    /// A signed 64-bit integer value.
    Integer,
    /// A boolean value.
    Boolean,
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text => f.write_str("TEXT"),
            Self::Integer => f.write_str("INTEGER"),
            Self::Boolean => f.write_str("BOOLEAN"),
        }
    }
}

/// A column in a persisted table schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SchemaColumn {
    pub(crate) name: String,
    pub(crate) data_type: DataType,
    pub(crate) nullable: bool,
}

/// A typed database value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// A non-null `TEXT` value.
    Text(String),
    /// A non-null `INTEGER` value.
    Integer(i64),
    /// A non-null `BOOLEAN` value.
    Boolean(bool),
    /// SQL `NULL`.
    Null,
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(value) => f.write_str(value),
            Self::Integer(value) => write!(f, "{value}"),
            Self::Boolean(value) => write!(f, "{value}"),
            Self::Null => f.write_str("NULL"),
        }
    }
}

/// Engine-produced provenance for a projected result column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnOrigin {
    table: String,
    column: String,
}

impl ColumnOrigin {
    pub(crate) fn new(table: String, column: String) -> Self {
        Self { table, column }
    }

    /// The source table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    /// The source column name.
    #[must_use]
    pub fn column(&self) -> &str {
        &self.column
    }
}

/// Engine-produced metadata for one projected result column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultColumn {
    label: String,
    origin: ColumnOrigin,
    data_type: DataType,
    nullable: bool,
}

impl ResultColumn {
    pub(crate) fn new(
        label: String,
        origin: ColumnOrigin,
        data_type: DataType,
        nullable: bool,
    ) -> Self {
        Self {
            label,
            origin,
            data_type,
            nullable,
        }
    }

    /// The display label for this result column.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The table column that supplied this result column.
    #[must_use]
    pub fn origin(&self) -> &ColumnOrigin {
        &self.origin
    }

    /// The SQL data type of this result column.
    #[must_use]
    pub const fn data_type(&self) -> DataType {
        self.data_type
    }

    /// Whether the source column was declared nullable.
    ///
    /// This describes schema metadata, not the values in this particular
    /// result. Predicates and inner joins may eliminate all `NULL` values.
    #[must_use]
    pub const fn nullable(&self) -> bool {
        self.nullable
    }
}

/// An immutable, materialized snapshot returned by a `SELECT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowSet {
    columns: Vec<ResultColumn>,
    rows: Vec<Vec<Value>>,
}

impl RowSet {
    pub(crate) fn new(columns: Vec<ResultColumn>, rows: Vec<Vec<Value>>) -> Self {
        debug_assert!(
            rows.iter().all(|row| row.len() == columns.len()),
            "every result row must match the projected column width"
        );
        Self { columns, rows }
    }

    /// Projected columns, in query order and including duplicates.
    #[must_use]
    pub fn columns(&self) -> &[ResultColumn] {
        &self.columns
    }

    /// Materialized rows in deterministic query order.
    #[must_use]
    pub fn rows(&self) -> &[Vec<Value>] {
        &self.rows
    }

    /// Consume this result and return its materialized rows.
    #[must_use]
    pub fn into_rows(self) -> Vec<Vec<Value>> {
        self.rows
    }

    /// Consume this result and return its column metadata and rows.
    #[must_use]
    pub fn into_parts(self) -> (Vec<ResultColumn>, Vec<Vec<Value>>) {
        (self.columns, self.rows)
    }
}

/// An immutable explanation of the source-row scan produced for a `SELECT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectExplanation {
    pattern: String,
    sources: Vec<String>,
    columns: Vec<ResultColumn>,
}

impl SelectExplanation {
    pub(crate) fn new(pattern: String, sources: Vec<String>, columns: Vec<ResultColumn>) -> Self {
        Self {
            pattern,
            sources,
            columns,
        }
    }

    /// The generated pattern used to select complete encoded rows.
    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Source tables in `FROM`/`JOIN` order.
    #[must_use]
    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    /// Projected columns, in query order and including duplicates.
    #[must_use]
    pub fn columns(&self) -> &[ResultColumn] {
        &self.columns
    }
}

/// The result of executing one statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A materialized `SELECT` result.
    Rows(RowSet),
    /// The result of an `INSERT`, `UPDATE`, or `DELETE`.
    Affected {
        /// Number of inserted, updated, or deleted rows.
        rows: usize,
    },
    /// The result of `CREATE TABLE`.
    Created {
        /// Normalized name of the created table.
        table: String,
    },
    /// A compiled `EXPLAIN REGEX` result.
    Explain(SelectExplanation),
}

impl Outcome {
    /// Whether this result came from a statement that mutates the database.
    #[must_use]
    pub const fn is_mutation(&self) -> bool {
        matches!(self, Self::Affected { .. } | Self::Created { .. })
    }
}

pub(crate) fn validate_value(value: &Value, column: &SchemaColumn) -> Result<()> {
    match (value, column.data_type) {
        (Value::Null, _) if column.nullable => Ok(()),
        (Value::Null, _) => Err(Error::type_error(format!(
            "column {:?} is NOT NULL",
            column.name
        ))),
        (Value::Text(_), DataType::Text)
        | (Value::Integer(_), DataType::Integer)
        | (Value::Boolean(_), DataType::Boolean) => Ok(()),
        (actual, expected) => Err(Error::type_error(format!(
            "column {:?} expects {expected}, got {}",
            column.name,
            value_kind(actual)
        ))),
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

#[cfg(test)]
mod tests;
