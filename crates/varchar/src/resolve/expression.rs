//! Stack-safe semantic resolution of Boolean-expression programs.

use super::column::{require_local_column, resolve_column};
use crate::expression::{Predicate as ResolvedPredicate, Program, ProgramNode, compile_pattern};
use crate::limits::check_limit;
#[cfg(test)]
use crate::sql::Predicate;
use crate::sql::{Expression, ExpressionNode, PredicateOperator};
use crate::storage::TableSchema;
use crate::value::validate_value;
use crate::{DataType, Error, Resource, Result, Value};

pub(crate) fn local_expression<'statement>(
    schema: &TableSchema,
    expression: Option<&'statement Expression>,
    max_predicates: usize,
) -> Result<Option<Program<'statement>>> {
    let Some(expression) = expression else {
        return Ok(None);
    };
    resolve_program(&[schema], expression, max_predicates, true).map(Some)
}

pub(crate) fn expression<'statement>(
    sources: &[&TableSchema],
    expression: Option<&'statement Expression>,
    max_predicates: usize,
) -> Result<Option<Program<'statement>>> {
    let Some(expression) = expression else {
        return Ok(None);
    };
    resolve_program(sources, expression, max_predicates, false).map(Some)
}

#[cfg(test)]
pub(crate) fn predicate<'statement>(
    schema: &TableSchema,
    predicate: &'statement Predicate,
) -> Result<ResolvedPredicate<'statement>> {
    let column = require_local_column(schema, &predicate.column)?;
    predicate_at(
        schema,
        super::ColumnLocation { source: 0, column },
        &predicate.operator,
    )
}

fn resolve_program<'statement>(
    sources: &[&TableSchema],
    expression: &'statement Expression,
    max_predicates: usize,
    local: bool,
) -> Result<Program<'statement>> {
    check_limit(
        expression.predicate_units()?,
        max_predicates,
        Resource::WherePredicates,
    )?;

    let mut nodes = Vec::new();
    nodes
        .try_reserve_exact(expression.nodes().len())
        .map_err(|_| Error::Allocation {
            operation: "reserving a resolved expression program",
        })?;
    for node in expression.nodes() {
        nodes.push(match node {
            ExpressionNode::And { children } => ProgramNode::And {
                children: *children,
            },
            ExpressionNode::Or { children } => ProgramNode::Or {
                children: *children,
            },
            ExpressionNode::Predicate(predicate) => {
                let location = if local {
                    super::ColumnLocation {
                        source: 0,
                        column: require_local_column(sources[0], &predicate.column)?,
                    }
                } else {
                    resolve_column(sources, &predicate.column)?
                };
                ProgramNode::Predicate(predicate_at(
                    sources[location.source],
                    location,
                    &predicate.operator,
                )?)
            }
        });
    }
    Ok(Program::new(nodes))
}

fn predicate_at<'statement>(
    schema: &TableSchema,
    column: super::ColumnLocation,
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
        PredicateOperator::LessThan(Value::Null)
        | PredicateOperator::LessThanOrEqual(Value::Null)
        | PredicateOperator::GreaterThan(Value::Null)
        | PredicateOperator::GreaterThanOrEqual(Value::Null) => Err(Error::Type(String::from(
            "NULL cannot be compared with `<`, `<=`, `>`, or `>=`; use IS NULL or IS NOT NULL",
        ))),
        PredicateOperator::LessThan(value) => {
            validate_value(value, definition)?;
            Ok(ResolvedPredicate::LessThan { column, value })
        }
        PredicateOperator::LessThanOrEqual(value) => {
            validate_value(value, definition)?;
            Ok(ResolvedPredicate::LessThanOrEqual { column, value })
        }
        PredicateOperator::GreaterThan(value) => {
            validate_value(value, definition)?;
            Ok(ResolvedPredicate::GreaterThan { column, value })
        }
        PredicateOperator::GreaterThanOrEqual(value) => {
            validate_value(value, definition)?;
            Ok(ResolvedPredicate::GreaterThanOrEqual { column, value })
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
