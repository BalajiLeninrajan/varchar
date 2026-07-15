//! Schema-aware resolution of row predicates.

use super::column::require_column;
use crate::sql::{Predicate, PredicateOperator};
use crate::storage::TableSchema;
use crate::value::validate_value;
use crate::{DataType, Error, Result, Value};

pub(crate) enum ResolvedPredicate<'a> {
    Equal { column: usize, value: &'a Value },
    NotEqual { column: usize, value: &'a Value },
    Like { column: usize, pattern: &'a str },
    IsNull { column: usize },
    IsNotNull { column: usize },
}

pub(crate) fn predicate<'a>(
    schema: &TableSchema,
    predicate: &'a Predicate,
) -> Result<ResolvedPredicate<'a>> {
    let column = require_column(schema, &predicate.column)?;
    let definition = &schema.columns[column];
    match &predicate.operator {
        PredicateOperator::Equal(Value::Null) | PredicateOperator::NotEqual(Value::Null) => {
            Err(Error::Type(String::from(
                "NULL cannot be compared with `=` or `!=`; use IS NULL or IS NOT NULL",
            )))
        }
        PredicateOperator::Equal(value) => {
            validate_value(value, definition)?;
            Ok(ResolvedPredicate::Equal { column, value })
        }
        PredicateOperator::NotEqual(value) => {
            validate_value(value, definition)?;
            Ok(ResolvedPredicate::NotEqual { column, value })
        }
        PredicateOperator::Like(pattern) => {
            if definition.data_type != DataType::Text {
                return Err(Error::Type(format!(
                    "LIKE requires a TEXT column; {:?} is {}",
                    definition.name, definition.data_type
                )));
            }
            Ok(ResolvedPredicate::Like { column, pattern })
        }
        PredicateOperator::IsNull => Ok(ResolvedPredicate::IsNull { column }),
        PredicateOperator::IsNotNull => Ok(ResolvedPredicate::IsNotNull { column }),
    }
}
