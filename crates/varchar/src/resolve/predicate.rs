//! Schema-aware resolution of row predicates.

use super::column::{require_local_column, resolve_column};
use super::like::{LikeAtom, resolve_like_pattern};
use crate::sql::{Predicate, PredicateOperator};
use crate::storage::TableSchema;
use crate::value::validate_value;
use crate::{DataType, Error, Result, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResolvedPredicate<'a> {
    Equal { column: usize, value: &'a Value },
    NotEqual { column: usize, value: &'a Value },
    Like { column: usize, atoms: Vec<LikeAtom> },
    IsNull { column: usize },
    IsNotNull { column: usize },
}

pub(crate) struct ResolvedSourcePredicate<'a> {
    pub(crate) source: usize,
    pub(crate) predicate: ResolvedPredicate<'a>,
}

pub(crate) fn predicate<'a>(
    schema: &TableSchema,
    predicate: &'a Predicate,
) -> Result<ResolvedPredicate<'a>> {
    let column = require_local_column(schema, &predicate.column)?;
    predicate_at(schema, column, &predicate.operator)
}

pub(super) fn resolve_select_predicate<'a>(
    sources: &[&TableSchema],
    predicate: &'a Predicate,
) -> Result<ResolvedSourcePredicate<'a>> {
    let location = resolve_column(sources, &predicate.column)?;
    Ok(ResolvedSourcePredicate {
        source: location.source,
        predicate: predicate_at(
            sources[location.source],
            location.column,
            &predicate.operator,
        )?,
    })
}

fn predicate_at<'a>(
    schema: &TableSchema,
    column: usize,
    operator: &'a PredicateOperator,
) -> Result<ResolvedPredicate<'a>> {
    let definition = &schema.columns[column];
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
