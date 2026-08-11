//! Schema-aware resolution of row predicates.

use super::column::{ColumnLocation, require_local_column, resolve_column};
use super::like::{LikeAtom, resolve_like_pattern};
use crate::sql::{Predicate, PredicateOperator};
use crate::storage::TableSchema;
use crate::value::validate_value;
use crate::{DataType, Error, Result, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResolvedPredicate<'a> {
    Equal {
        column: ColumnLocation,
        value: &'a Value,
    },
    NotEqual {
        column: ColumnLocation,
        value: &'a Value,
    },
    Like {
        column: ColumnLocation,
        atoms: Vec<LikeAtom>,
    },
    IsNull {
        column: ColumnLocation,
    },
    IsNotNull {
        column: ColumnLocation,
    },
}

impl ResolvedPredicate<'_> {
    pub(crate) const fn column(&self) -> ColumnLocation {
        match self {
            Self::Equal { column, .. }
            | Self::NotEqual { column, .. }
            | Self::Like { column, .. }
            | Self::IsNull { column }
            | Self::IsNotNull { column } => *column,
        }
    }
}

pub(crate) fn predicate<'a>(
    schema: &TableSchema,
    predicate: &'a Predicate,
) -> Result<ResolvedPredicate<'a>> {
    let column = require_local_column(schema, &predicate.column)?;
    predicate_at(
        schema,
        ColumnLocation { source: 0, column },
        &predicate.operator,
    )
}

pub(super) fn resolve_select_predicate<'a>(
    sources: &[&TableSchema],
    predicate: &'a Predicate,
) -> Result<ResolvedPredicate<'a>> {
    let location = resolve_column(sources, &predicate.column)?;
    predicate_at(sources[location.source], location, &predicate.operator)
}

fn predicate_at<'a>(
    schema: &TableSchema,
    column: ColumnLocation,
    operator: &'a PredicateOperator,
) -> Result<ResolvedPredicate<'a>> {
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
                atoms: resolve_like_pattern(pattern)?,
            })
        }
        PredicateOperator::IsNull => Ok(ResolvedPredicate::IsNull { column }),
        PredicateOperator::IsNotNull => Ok(ResolvedPredicate::IsNotNull { column }),
    }
}
