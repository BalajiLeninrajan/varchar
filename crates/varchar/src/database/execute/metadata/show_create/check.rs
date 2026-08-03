use std::fmt;

use crate::expression::{CheckPredicate, CheckProgram, CheckProgramNode, LikeAtom, subtree_sizes};
use crate::storage::TableSchema;

use super::{FormatError, write_identifier, write_value};

pub(super) fn write(
    output: &mut impl fmt::Write,
    table: &TableSchema,
    program: &CheckProgram,
) -> Result<(), CheckFormatError> {
    let nodes = program.nodes();
    let sizes = subtree_sizes(nodes).map_err(check_shape_error)?;
    let event_capacity = nodes
        .len()
        .checked_mul(3)
        .ok_or(CheckFormatError::Allocation)?;
    let mut events = Vec::new();
    events
        .try_reserve_exact(event_capacity)
        .map_err(|_| CheckFormatError::Allocation)?;
    let mut children = Vec::new();
    children
        .try_reserve_exact(nodes.len())
        .map_err(|_| CheckFormatError::Allocation)?;
    events.push(Event::Node(0));

    while let Some(event) = events.pop() {
        match event {
            Event::Node(index) => match nodes.get(index).ok_or(CheckFormatError::Write)? {
                CheckProgramNode::Predicate(predicate) => {
                    write_predicate(output, table, predicate)?;
                }
                CheckProgramNode::And { children: count }
                | CheckProgramNode::Or { children: count } => {
                    let separator = match nodes[index] {
                        CheckProgramNode::And { .. } => " AND ",
                        CheckProgramNode::Or { .. } => " OR ",
                        CheckProgramNode::Predicate(_) => unreachable!(),
                    };
                    output
                        .write_char('(')
                        .map_err(|_| CheckFormatError::Write)?;
                    events.push(Event::CloseParen);
                    children.clear();
                    let mut child = index.checked_add(1).ok_or(CheckFormatError::Write)?;
                    for _ in 0..*count {
                        children.push(child);
                        child = child
                            .checked_add(*sizes.get(child).ok_or(CheckFormatError::Write)?)
                            .ok_or(CheckFormatError::Write)?;
                    }
                    for (position, child) in children.iter().enumerate().rev() {
                        events.push(Event::Node(*child));
                        if position > 0 {
                            events.push(Event::Separator(separator));
                        }
                    }
                }
            },
            Event::Separator(separator) => output
                .write_str(separator)
                .map_err(|_| CheckFormatError::Write)?,
            Event::CloseParen => output
                .write_char(')')
                .map_err(|_| CheckFormatError::Write)?,
        }
    }
    Ok(())
}

/// Map a shared subtree measurement failure onto this writer's error kind.
fn check_shape_error(error: crate::Error) -> CheckFormatError {
    match error {
        crate::Error::Allocation { .. } => CheckFormatError::Allocation,
        _ => CheckFormatError::Write,
    }
}

fn write_predicate(
    output: &mut impl fmt::Write,
    table: &TableSchema,
    predicate: &CheckPredicate,
) -> Result<(), CheckFormatError> {
    let column = table
        .columns
        .get(predicate.column())
        .ok_or(CheckFormatError::Write)?;
    map_value(write_identifier(output, &column.name))?;
    match predicate {
        CheckPredicate::Equal { value, .. } => write_comparison(output, " = ", value),
        CheckPredicate::NotEqual { value, .. } => write_comparison(output, " != ", value),
        CheckPredicate::LessThan { value, .. } => write_comparison(output, " < ", value),
        CheckPredicate::LessThanOrEqual { value, .. } => write_comparison(output, " <= ", value),
        CheckPredicate::GreaterThan { value, .. } => write_comparison(output, " > ", value),
        CheckPredicate::GreaterThanOrEqual { value, .. } => write_comparison(output, " >= ", value),
        CheckPredicate::Like { atoms, .. } => {
            output
                .write_str(" LIKE ")
                .map_err(|_| CheckFormatError::Write)?;
            write_pattern(output, atoms)
        }
        CheckPredicate::IsNull { .. } => output
            .write_str(" IS NULL")
            .map_err(|_| CheckFormatError::Write),
        CheckPredicate::IsNotNull { .. } => output
            .write_str(" IS NOT NULL")
            .map_err(|_| CheckFormatError::Write),
        CheckPredicate::In { values, .. } => {
            output
                .write_str(" IN (")
                .map_err(|_| CheckFormatError::Write)?;
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output
                        .write_str(", ")
                        .map_err(|_| CheckFormatError::Write)?;
                }
                map_value(write_value(output, value))?;
            }
            output.write_char(')').map_err(|_| CheckFormatError::Write)
        }
    }
}

fn write_comparison(
    output: &mut impl fmt::Write,
    operator: &str,
    value: &crate::Value,
) -> Result<(), CheckFormatError> {
    output
        .write_str(operator)
        .map_err(|_| CheckFormatError::Write)?;
    map_value(write_value(output, value))
}

fn write_pattern(output: &mut impl fmt::Write, atoms: &[LikeAtom]) -> Result<(), CheckFormatError> {
    output
        .write_char('\'')
        .map_err(|_| CheckFormatError::Write)?;
    for atom in atoms {
        match atom {
            LikeAtom::AnySequence => output
                .write_char('%')
                .map_err(|_| CheckFormatError::Write)?,
            LikeAtom::AnyScalar => output
                .write_char('_')
                .map_err(|_| CheckFormatError::Write)?,
            LikeAtom::Literal('%') => output
                .write_str("\\%")
                .map_err(|_| CheckFormatError::Write)?,
            LikeAtom::Literal('_') => output
                .write_str("\\_")
                .map_err(|_| CheckFormatError::Write)?,
            LikeAtom::Literal('\\') => output
                .write_str("\\\\")
                .map_err(|_| CheckFormatError::Write)?,
            LikeAtom::Literal('\'') => output
                .write_str("''")
                .map_err(|_| CheckFormatError::Write)?,
            LikeAtom::Literal(character) => output
                .write_char(*character)
                .map_err(|_| CheckFormatError::Write)?,
        }
    }
    output.write_char('\'').map_err(|_| CheckFormatError::Write)
}

fn map_value(result: Result<(), FormatError>) -> Result<(), CheckFormatError> {
    result.map_err(|error| match error {
        FormatError::Write => CheckFormatError::Write,
        FormatError::Allocation => CheckFormatError::Allocation,
    })
}

enum Event {
    Node(usize),
    Separator(&'static str),
    CloseParen,
}

pub(super) enum CheckFormatError {
    Write,
    Allocation,
}
