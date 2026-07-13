use std::fmt;

use crate::{Error, Result};

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
