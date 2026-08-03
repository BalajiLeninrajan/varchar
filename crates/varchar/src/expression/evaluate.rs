//! Iterative evaluation of resolved expression programs.

use crate::resolve::ColumnLocation;
use crate::{Error, Result, Value};

use super::program::{Predicate, Program, ProgramNode};
use super::truth::Truth;

struct Frame {
    remaining: usize,
    value: Truth,
}

#[derive(Clone, Copy)]
enum EvaluationRows<'rows, 'values> {
    Joined(&'rows [&'values [Value]]),
    Local {
        source: usize,
        row: &'values [Value],
    },
}

/// Reusable, fallibly allocated state for row-value evaluation.
pub(crate) struct Evaluator {
    frames: Vec<Frame>,
}

impl Evaluator {
    pub(crate) fn working_bytes(program: &Program<'_>) -> Result<usize> {
        program
            .logical_node_count()
            .checked_mul(std::mem::size_of::<Frame>())
            .ok_or(Error::Capacity {
                operation: "sizing the expression evaluation stack",
            })
    }

    /// Reserve evaluation state for `program`.
    pub(crate) fn new(program: &Program<'_>) -> Result<Self> {
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(program.logical_node_count())
            .map_err(|_| Error::Allocation {
                operation: "reserving the expression evaluation stack",
            })?;
        Ok(Self { frames })
    }

    pub(crate) fn evaluate_where(
        &mut self,
        program: &Program<'_>,
        rows: &[&[Value]],
    ) -> Result<bool> {
        self.evaluate(program, EvaluationRows::Joined(rows))
            .map(Truth::passes_where)
    }

    pub(crate) fn evaluate_where_local(
        &mut self,
        program: &Program<'_>,
        source: usize,
        row: &[Value],
    ) -> Result<bool> {
        self.evaluate(program, EvaluationRows::Local { source, row })
            .map(Truth::passes_where)
    }

    fn evaluate(&mut self, program: &Program<'_>, rows: EvaluationRows<'_, '_>) -> Result<Truth> {
        self.frames.clear();
        if self.frames.capacity() < program.logical_node_count() {
            return Err(Error::Capacity {
                operation: "evaluating an expression with an undersized stack",
            });
        }
        let nodes = program.nodes();
        let mut cursor = 0_usize;

        loop {
            let node = nodes.get(cursor).ok_or(Error::Capacity {
                operation: "evaluating a resolved expression program",
            })?;
            cursor = cursor.checked_add(1).ok_or(Error::Capacity {
                operation: "advancing through a resolved expression program",
            })?;

            let mut value = match node {
                ProgramNode::And { children } => {
                    self.frames.push(Frame {
                        remaining: *children,
                        value: Truth::True,
                    });
                    continue;
                }
                ProgramNode::Predicate(predicate) => evaluate_predicate(predicate, rows)?,
            };

            loop {
                let Some(frame) = self.frames.last_mut() else {
                    if cursor != nodes.len() {
                        return Err(Error::Capacity {
                            operation: "finishing a resolved expression program",
                        });
                    }
                    return Ok(value);
                };

                frame.remaining = frame.remaining.checked_sub(1).ok_or(Error::Capacity {
                    operation: "counting evaluated expression children",
                })?;
                frame.value = frame.value.and(value);
                let short_circuits = matches!(frame.value, Truth::False);
                let skipped = if short_circuits { frame.remaining } else { 0 };
                if skipped > 0 {
                    frame.remaining = 0;
                    cursor = skip_subtrees(nodes, cursor, skipped)?;
                }

                if frame.remaining > 0 {
                    break;
                }
                value = frame.value;
                self.frames.pop();
            }
        }
    }
}

fn evaluate_predicate(predicate: &Predicate<'_>, rows: EvaluationRows<'_, '_>) -> Result<Truth> {
    let left = value_at(rows, predicate.column())?;
    Ok(match predicate {
        Predicate::Equal { value, .. } => compare_equal(left, value),
        Predicate::NotEqual { value, .. } => match compare_equal(left, value) {
            Truth::True => Truth::False,
            Truth::False => Truth::True,
            Truth::Unknown => Truth::Unknown,
        },
        Predicate::Like { .. } => {
            return Err(Error::Type(String::from(
                "residual LIKE evaluation is not available",
            )));
        }
        Predicate::IsNull { .. } => truth(matches!(left, Value::Null)),
        Predicate::IsNotNull { .. } => truth(!matches!(left, Value::Null)),
    })
}

fn value_at<'values>(
    rows: EvaluationRows<'_, 'values>,
    location: ColumnLocation,
) -> Result<&'values Value> {
    let value = match rows {
        EvaluationRows::Joined(rows) => rows
            .get(location.source)
            .and_then(|row| row.get(location.column)),
        EvaluationRows::Local { source, row } if source == location.source => {
            row.get(location.column)
        }
        EvaluationRows::Local { .. } => None,
    };
    value.ok_or_else(|| {
        Error::Schema(format!(
            "resolved expression column {}.{} is outside the evaluated rows",
            location.source, location.column
        ))
    })
}

fn compare_equal(left: &Value, right: &Value) -> Truth {
    if matches!(left, Value::Null) {
        Truth::Unknown
    } else if values_equal(left, right) {
        Truth::True
    } else {
        Truth::False
    }
}

fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Text(left), Value::Text(right)) => left == right,
        (Value::Integer(left), Value::Integer(right)) => left == right,
        (Value::Boolean(left), Value::Boolean(right)) => left == right,
        (Value::Null, Value::Null) => true,
        (
            Value::Text(_) | Value::Integer(_) | Value::Boolean(_) | Value::Null,
            Value::Text(_) | Value::Integer(_) | Value::Boolean(_) | Value::Null,
        ) => false,
    }
}

const fn truth(value: bool) -> Truth {
    if value { Truth::True } else { Truth::False }
}

fn skip_subtrees(nodes: &[ProgramNode<'_>], mut cursor: usize, count: usize) -> Result<usize> {
    let mut pending = count;
    while pending > 0 {
        let node = nodes.get(cursor).ok_or(Error::Capacity {
            operation: "short-circuiting a resolved expression program",
        })?;
        pending = pending.checked_sub(1).ok_or(Error::Capacity {
            operation: "counting skipped expression children",
        })?;
        pending = pending
            .checked_add(node.child_count())
            .ok_or(Error::Capacity {
                operation: "counting skipped expression descendants",
            })?;
        cursor = cursor.checked_add(1).ok_or(Error::Capacity {
            operation: "advancing over skipped expression descendants",
        })?;
    }
    Ok(cursor)
}
