use std::fmt;

use crate::{Error, ExplainPlan, Result};

/// A column's SQL type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataType {
    Text,
    Integer,
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

/// The table column from which a result column originated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnOrigin {
    table: String,
    column: String,
}

impl ColumnOrigin {
    /// Construct source-column provenance.
    #[must_use]
    pub fn new(table: String, column: String) -> Self {
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

/// Metadata for one projected result column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultColumn {
    label: String,
    origin: ColumnOrigin,
    data_type: DataType,
    nullable: bool,
}

impl ResultColumn {
    /// Construct projected-column metadata.
    #[must_use]
    pub fn new(label: String, origin: ColumnOrigin, data_type: DataType, nullable: bool) -> Self {
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

    /// Whether this result column can contain `NULL`.
    #[must_use]
    pub const fn nullable(&self) -> bool {
        self.nullable
    }
}

/// A typed database value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Text(String),
    Integer(i64),
    Boolean(bool),
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

pub(crate) fn validate_value(value: &Value, column: &SchemaColumn) -> Result<()> {
    match (value, column.data_type) {
        (Value::Null, _) if column.nullable => Ok(()),
        (Value::Null, _) => Err(Error::Type(format!("column {:?} is NOT NULL", column.name))),
        (Value::Text(_), DataType::Text)
        | (Value::Integer(_), DataType::Integer)
        | (Value::Boolean(_), DataType::Boolean) => Ok(()),
        (actual, expected) => Err(Error::Type(format!(
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

/// Rows returned by a `SELECT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowSet {
    columns: Vec<ResultColumn>,
    rows: Vec<Vec<Value>>,
}

impl RowSet {
    /// Construct a materialized result set.
    #[must_use]
    pub fn new(columns: Vec<ResultColumn>, rows: Vec<Vec<Value>>) -> Self {
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

/// The result of executing one statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Rows(RowSet),
    Affected { rows: usize },
    Created { table: String },
    Explain(ExplainPlan),
}

impl Outcome {
    /// Whether this result came from a statement that mutates the database.
    #[must_use]
    pub const fn is_mutation(&self) -> bool {
        matches!(self, Self::Affected { .. } | Self::Created { .. })
    }
}
