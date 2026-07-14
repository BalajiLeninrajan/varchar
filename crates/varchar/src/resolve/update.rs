//! `UPDATE` assignment resolution, validation, and sequence advancement.

use std::collections::BTreeSet;

use super::column::require_column;
use crate::sql::Assignment;
use crate::storage::{AutoIncrement, TableSchema};
use crate::value::validate_value;
use crate::{Error, Result, Value};

pub(crate) struct ResolvedAssignments {
    pub(crate) values: Vec<(usize, Value)>,
    pub(crate) next_auto_increment: Option<i64>,
}

pub(crate) fn assignments(
    schema: &TableSchema,
    auto_increment: Option<AutoIncrement>,
    assignments: &[Assignment],
) -> Result<ResolvedAssignments> {
    let mut resolved = Vec::with_capacity(assignments.len());
    let mut seen = BTreeSet::new();
    for assignment in assignments {
        if !seen.insert(assignment.column.as_str()) {
            return Err(Error::schema(format!(
                "duplicate UPDATE assignment for column {:?}",
                assignment.column
            )));
        }
        let index = require_column(schema, &assignment.column)?;
        validate_value(&assignment.value, &schema.columns[index])?;
        resolved.push((index, assignment.value.clone()));
    }
    let next_auto_increment = auto_increment.and_then(|auto_increment| {
        resolved
            .iter()
            .find(|(column, _)| *column == auto_increment.column)
            .and_then(|(_, value)| match value {
                Value::Integer(value) if *value > auto_increment.last => Some(*value),
                Value::Integer(_) | Value::Text(_) | Value::Boolean(_) | Value::Null => None,
            })
    });
    Ok(ResolvedAssignments {
        values: resolved,
        next_auto_increment,
    })
}
