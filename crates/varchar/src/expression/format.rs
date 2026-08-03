//! Stack-safe formatting for normalized parsed expressions.

use std::fmt::{self, Write};

use super::subtree_sizes;
use crate::Value;
use crate::sql::{Expression, ExpressionNode, Predicate, PredicateOperator};

impl fmt::Display for Expression {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let nodes = self.nodes();
        let sizes = subtree_sizes(nodes).map_err(|_| fmt::Error)?;
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
        PredicateOperator::In(values) => {
            formatter.write_str(" IN (")?;
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    formatter.write_str(", ")?;
                }
                format_value(formatter, value)?;
            }
            formatter.write_char(')')
        }
    }
}

/// Writes `value` as the SQL literal that parses back to it.
///
/// This is the crate's only literal renderer: `Display for Expression` uses it
/// against a [`fmt::Formatter`], and result construction uses it against a
/// pre-sized [`String`], so it is generic over the sink rather than tied to
/// either one.
pub(crate) fn format_value<W: fmt::Write + ?Sized>(writer: &mut W, value: &Value) -> fmt::Result {
    match value {
        Value::Text(value) => format_text(writer, value),
        Value::Integer(value) => write!(writer, "{value}"),
        Value::Boolean(true) => writer.write_str("TRUE"),
        Value::Boolean(false) => writer.write_str("FALSE"),
        Value::Null => writer.write_str("NULL"),
    }
}

/// Writes `value` as a quoted SQL text literal, doubling embedded apostrophes.
pub(crate) fn format_text<W: fmt::Write + ?Sized>(writer: &mut W, value: &str) -> fmt::Result {
    writer.write_char('\'')?;
    for character in value.chars() {
        if character == '\'' {
            writer.write_str("''")?;
        } else {
            writer.write_char(character)?;
        }
    }
    writer.write_char('\'')
}

/// Byte length of what [`format_value`] writes for `value`.
///
/// Callers reserve exactly this much before rendering, so it counts the same
/// bytes the writer emits by running the writer against a sink that only
/// measures. Quoting and apostrophe doubling therefore cannot drift out of
/// step with the renderer.
pub(crate) fn format_value_len(value: &Value) -> usize {
    let mut counter = LengthCounter { bytes: 0 };
    format_value(&mut counter, value).expect("measuring a literal never fails");
    counter.bytes
}

/// A [`fmt::Write`] sink that keeps only the byte length written to it.
struct LengthCounter {
    bytes: usize,
}

impl fmt::Write for LengthCounter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        // A literal's length is bounded by the values already held in memory,
        // so the saturation is unreachable rather than a silent truncation.
        self.bytes = self.bytes.saturating_add(text.len());
        Ok(())
    }
}
