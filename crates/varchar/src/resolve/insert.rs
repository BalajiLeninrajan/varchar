//! `INSERT` column placement, value validation, and sequence advancement.

use std::collections::BTreeSet;

use super::column::require_column;
use crate::storage::{AutoIncrement, TableSchema};
use crate::value::validate_value;
use crate::{Error, Result, Value};

pub(crate) struct ResolvedInsert {
    pub(crate) values: Vec<Value>,
    pub(crate) next_auto_increment: Option<i64>,
}

pub(crate) fn insert_values(
    schema: &TableSchema,
    auto_increment: Option<AutoIncrement>,
    columns: Option<Vec<String>>,
    supplied: Vec<Value>,
) -> Result<ResolvedInsert> {
    let mut values = if let Some(columns) = columns {
        if columns.len() != supplied.len() {
            return Err(Error::type_error(format!(
                "INSERT names {} columns but supplies {} values",
                columns.len(),
                supplied.len()
            )));
        }
        let mut seen = BTreeSet::new();
        let mut values = vec![Value::Null; schema.columns.len()];
        for (name, value) in columns.into_iter().zip(supplied) {
            if !seen.insert(name.clone()) {
                return Err(Error::schema(format!("duplicate INSERT column {name:?}")));
            }
            let index = require_column(schema, &name)?;
            values[index] = value;
        }
        values
    } else {
        if supplied.len() != schema.columns.len() {
            return Err(Error::type_error(format!(
                "table {:?} expects {} values, got {}",
                schema.name,
                schema.columns.len(),
                supplied.len()
            )));
        }
        supplied
    };

    let next_auto_increment = if let Some(auto_increment) = auto_increment {
        let value = values
            .get_mut(auto_increment.column)
            .expect("validated auto-increment column is in the schema");
        match value {
            Value::Null => {
                let next = auto_increment.last.checked_add(1).ok_or_else(|| {
                    Error::constraint(format!(
                        "auto-increment sequence for table {:?} is exhausted",
                        schema.name
                    ))
                })?;
                *value = Value::Integer(next);
                Some(next)
            }
            Value::Integer(value) if *value > auto_increment.last => Some(*value),
            Value::Integer(_) | Value::Text(_) | Value::Boolean(_) => None,
        }
    } else {
        None
    };

    for (value, column) in values.iter().zip(&schema.columns) {
        validate_value(value, column)?;
    }
    Ok(ResolvedInsert {
        values,
        next_auto_increment,
    })
}
