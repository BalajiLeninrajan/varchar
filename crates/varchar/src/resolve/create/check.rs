//! Resolution of table-local CHECK declarations into owned flat programs.

use crate::expression::{CheckPredicate, CheckProgram, CheckProgramNode, compile_pattern};
use crate::sql::{Expression, ExpressionNode, Predicate, PredicateOperator};
use crate::{DataType, Error, Result, SchemaColumn, Value};

pub(super) fn resolve_check(
    table: &str,
    columns: &[SchemaColumn],
    expression: &Expression,
) -> Result<CheckProgram> {
    let mut nodes = Vec::new();
    nodes
        .try_reserve_exact(expression.nodes().len())
        .map_err(|_| Error::Allocation {
            operation: "reserving a resolved CHECK program",
        })?;
    for node in expression.nodes() {
        nodes.push(match node {
            ExpressionNode::And { children } => CheckProgramNode::And {
                children: *children,
            },
            ExpressionNode::Or { children } => CheckProgramNode::Or {
                children: *children,
            },
            ExpressionNode::Predicate(predicate) => {
                CheckProgramNode::Predicate(resolve_predicate(table, columns, predicate)?)
            }
        });
    }
    Ok(CheckProgram::new(nodes))
}

fn resolve_predicate(
    table: &str,
    columns: &[SchemaColumn],
    predicate: &Predicate,
) -> Result<CheckPredicate> {
    if predicate.column.qualifier.is_some() {
        return Err(Error::Schema(format!(
            "CHECK references must be unqualified local columns; {:?} is qualified",
            predicate.column.name
        )));
    }
    let column = columns
        .iter()
        .position(|column| column.name == predicate.column.name)
        .ok_or_else(|| {
            Error::Schema(format!(
                "CHECK references unknown column {:?} in table {:?}",
                predicate.column.name, table
            ))
        })?;
    let definition = &columns[column];

    Ok(match &predicate.operator {
        PredicateOperator::Equal(value) => CheckPredicate::Equal {
            column,
            value: comparison_value(value, definition.data_type, &definition.name)?,
        },
        PredicateOperator::NotEqual(value) => CheckPredicate::NotEqual {
            column,
            value: comparison_value(value, definition.data_type, &definition.name)?,
        },
        PredicateOperator::LessThan(value) => CheckPredicate::LessThan {
            column,
            value: comparison_value(value, definition.data_type, &definition.name)?,
        },
        PredicateOperator::LessThanOrEqual(value) => CheckPredicate::LessThanOrEqual {
            column,
            value: comparison_value(value, definition.data_type, &definition.name)?,
        },
        PredicateOperator::GreaterThan(value) => CheckPredicate::GreaterThan {
            column,
            value: comparison_value(value, definition.data_type, &definition.name)?,
        },
        PredicateOperator::GreaterThanOrEqual(value) => CheckPredicate::GreaterThanOrEqual {
            column,
            value: comparison_value(value, definition.data_type, &definition.name)?,
        },
        PredicateOperator::Like(pattern) => {
            if definition.data_type != DataType::Text {
                return Err(Error::Type(format!(
                    "LIKE requires a TEXT column; {:?} is {}",
                    definition.name, definition.data_type
                )));
            }
            CheckPredicate::Like {
                column,
                atoms: compile_pattern(pattern)?,
            }
        }
        PredicateOperator::IsNull => CheckPredicate::IsNull { column },
        PredicateOperator::IsNotNull => CheckPredicate::IsNotNull { column },
        PredicateOperator::In(values) => {
            if values.is_empty() {
                return Err(Error::Schema(String::from(
                    "CHECK IN predicates require at least one item",
                )));
            }
            let mut resolved = Vec::new();
            resolved
                .try_reserve_exact(values.len())
                .map_err(|_| Error::Allocation {
                    operation: "reserving resolved CHECK IN items",
                })?;
            for value in values {
                if matches!(value, Value::Null) {
                    resolved.push(Value::Null);
                } else {
                    resolved.push(typed_value(value, definition.data_type, &definition.name)?);
                }
            }
            CheckPredicate::In {
                column,
                values: resolved,
            }
        }
    })
}

fn comparison_value(value: &Value, expected: DataType, column: &str) -> Result<Value> {
    if matches!(value, Value::Null) {
        return Err(Error::Type(String::from(
            "NULL cannot be used as a comparison operand; use IS NULL or IS NOT NULL",
        )));
    }
    typed_value(value, expected, column)
}

fn typed_value(value: &Value, expected: DataType, column: &str) -> Result<Value> {
    match (value, expected) {
        (Value::Text(value), DataType::Text) => {
            let mut cloned = String::new();
            cloned
                .try_reserve_exact(value.len())
                .map_err(|_| Error::Allocation {
                    operation: "allocating a resolved CHECK text operand",
                })?;
            cloned.push_str(value);
            Ok(Value::Text(cloned))
        }
        (Value::Integer(value), DataType::Integer) => Ok(Value::Integer(*value)),
        (Value::Boolean(value), DataType::Boolean) => Ok(Value::Boolean(*value)),
        (actual, _) => Err(Error::Type(format!(
            "CHECK column {column:?} expects {expected}, got {}",
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
