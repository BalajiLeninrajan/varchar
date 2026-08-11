//! Schema-aware semantic resolution of row predicates.

use super::column::{ColumnLocation, require_local_column, resolve_column};
use crate::expression::{Predicate as ResolvedPredicate, compile_pattern};
use crate::sql::{Predicate, PredicateOperator};
use crate::storage::TableSchema;
use crate::value::validate_value;
use crate::{DataType, Error, Result, Value};

pub(crate) fn predicate<'statement>(
    schema: &TableSchema,
    predicate: &'statement Predicate,
) -> Result<ResolvedPredicate<'statement>> {
    let column = require_local_column(schema, &predicate.column)?;
    predicate_at(
        schema,
        ColumnLocation { source: 0, column },
        &predicate.operator,
    )
}

pub(super) fn resolve_select_predicate<'statement>(
    sources: &[&TableSchema],
    predicate: &'statement Predicate,
) -> Result<ResolvedPredicate<'statement>> {
    let location = resolve_column(sources, &predicate.column)?;
    predicate_at(sources[location.source], location, &predicate.operator)
}

fn predicate_at<'statement>(
    schema: &TableSchema,
    column: ColumnLocation,
    operator: &'statement PredicateOperator,
) -> Result<ResolvedPredicate<'statement>> {
    let definition = &schema.columns[column.column];
    match operator {
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
            Ok(ResolvedPredicate::Like {
                column,
                atoms: compile_pattern(pattern)?,
            })
        }
        PredicateOperator::IsNull => Ok(ResolvedPredicate::IsNull { column }),
        PredicateOperator::IsNotNull => Ok(ResolvedPredicate::IsNotNull { column }),
    }
}
