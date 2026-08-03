//! Stack-safe formatting for normalized parsed expressions.

use std::fmt::{self, Write};

use crate::Value;
use crate::sql::{Expression, ExpressionNode, Predicate, PredicateOperator};

impl fmt::Display for Expression {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let nodes = self.nodes();
        let sizes = subtree_sizes(nodes)?;
        let mut events = Vec::new();
        let mut children = Vec::new();
        events
            .try_reserve_exact(nodes.len().saturating_mul(3))
            .map_err(|_| fmt::Error)?;
        children
            .try_reserve_exact(nodes.len())
            .map_err(|_| fmt::Error)?;
        events.push(Event::Node {
            index: 0,
            parent_precedence: 0,
        });

        while let Some(event) = events.pop() {
            match event {
                Event::Node {
                    index,
                    parent_precedence,
                } => match &nodes[index] {
                    ExpressionNode::Predicate(predicate) => {
                        format_predicate(formatter, predicate)?;
                    }
                    ExpressionNode::And { children: count }
                    | ExpressionNode::Or { children: count } => {
                        let (precedence, separator) = match nodes[index] {
                            ExpressionNode::And { .. } => (2, " AND "),
                            ExpressionNode::Or { .. } => (1, " OR "),
                            ExpressionNode::Predicate(_) => unreachable!(),
                        };
                        let parenthesized = precedence < parent_precedence;
                        if parenthesized {
                            formatter.write_char('(')?;
                            events.push(Event::CloseParen);
                        }

                        children.clear();
                        let mut child = index + 1;
                        for _ in 0..*count {
                            children.push(child);
                            child = child.checked_add(sizes[child]).ok_or(fmt::Error)?;
                        }
                        for (position, child) in children.iter().enumerate().rev() {
                            events.push(Event::Node {
                                index: *child,
                                parent_precedence: precedence,
                            });
                            if position > 0 {
                                events.push(Event::Separator(separator));
                            }
                        }
                    }
                },
                Event::Separator(separator) => formatter.write_str(separator)?,
                Event::CloseParen => formatter.write_char(')')?,
            }
        }
        Ok(())
    }
}

enum Event {
    Node { index: usize, parent_precedence: u8 },
    Separator(&'static str),
    CloseParen,
}

fn subtree_sizes(nodes: &[ExpressionNode]) -> Result<Vec<usize>, fmt::Error> {
    let mut sizes = Vec::new();
    sizes
        .try_reserve_exact(nodes.len())
        .map_err(|_| fmt::Error)?;
    sizes.resize(nodes.len(), 0);

    let mut pending = Vec::new();
    pending
        .try_reserve_exact(nodes.len())
        .map_err(|_| fmt::Error)?;
    for (index, node) in nodes.iter().enumerate().rev() {
        let size = match node {
            ExpressionNode::Predicate(_) => 1,
            ExpressionNode::And { children } | ExpressionNode::Or { children } => {
                if pending.len() < *children {
                    return Err(fmt::Error);
                }
                let mut size = 1_usize;
                for _ in 0..*children {
                    size = size
                        .checked_add(pending.pop().ok_or(fmt::Error)?)
                        .ok_or(fmt::Error)?;
                }
                size
            }
        };
        sizes[index] = size;
        pending.push(size);
    }
    if pending.len() != 1 {
        return Err(fmt::Error);
    }
    Ok(sizes)
}

fn format_predicate(formatter: &mut fmt::Formatter<'_>, predicate: &Predicate) -> fmt::Result {
    if let Some(qualifier) = &predicate.column.qualifier {
        formatter.write_str(qualifier)?;
        formatter.write_char('.')?;
    }
    formatter.write_str(&predicate.column.name)?;
    match &predicate.operator {
        PredicateOperator::Equal(value) => {
            formatter.write_str(" = ")?;
            format_value(formatter, value)
        }
        PredicateOperator::NotEqual(value) => {
            formatter.write_str(" != ")?;
            format_value(formatter, value)
        }
        PredicateOperator::LessThan(value) => {
            formatter.write_str(" < ")?;
            format_value(formatter, value)
        }
        PredicateOperator::LessThanOrEqual(value) => {
            formatter.write_str(" <= ")?;
            format_value(formatter, value)
        }
        PredicateOperator::GreaterThan(value) => {
            formatter.write_str(" > ")?;
            format_value(formatter, value)
        }
        PredicateOperator::GreaterThanOrEqual(value) => {
            formatter.write_str(" >= ")?;
            format_value(formatter, value)
        }
        PredicateOperator::Like(pattern) => {
            formatter.write_str(" LIKE ")?;
            format_text(formatter, pattern)
        }
        PredicateOperator::IsNull => formatter.write_str(" IS NULL"),
        PredicateOperator::IsNotNull => formatter.write_str(" IS NOT NULL"),
    }
}

fn format_value(formatter: &mut fmt::Formatter<'_>, value: &Value) -> fmt::Result {
    match value {
        Value::Text(value) => format_text(formatter, value),
        Value::Integer(value) => write!(formatter, "{value}"),
        Value::Boolean(true) => formatter.write_str("TRUE"),
        Value::Boolean(false) => formatter.write_str("FALSE"),
        Value::Null => formatter.write_str("NULL"),
    }
}

fn format_text(formatter: &mut fmt::Formatter<'_>, value: &str) -> fmt::Result {
    formatter.write_char('\'')?;
    for character in value.chars() {
        if character == '\'' {
            formatter.write_str("''")?;
        } else {
            formatter.write_char(character)?;
        }
    }
    formatter.write_char('\'')
}
