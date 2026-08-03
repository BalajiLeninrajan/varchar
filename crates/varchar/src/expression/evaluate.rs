//! Iterative evaluation of resolved expression programs.

use std::cmp::Ordering;

use crate::resolve::ColumnLocation;
use crate::{Error, Result, Value};

use super::like::{self, LikeWork};
use super::program::{Predicate, Program};
use super::tree::{LogicalOperator, Node};
use super::truth::Truth;

struct Frame {
    operator: LogicalOperator,
    remaining: usize,
    value: Truth,
}

/// The operation labels one pipeline reports its evaluation failures under.
///
/// The `WHERE` and `CHECK` pipelines run the same driver but keep their own
/// wording, so sharing the driver leaves every diagnostic unchanged.
#[derive(Clone, Copy)]
struct EvaluationLabels {
    undersized_stack: &'static str,
    read_node: &'static str,
    advance: &'static str,
    finish: &'static str,
    count_children: &'static str,
    skip_node: &'static str,
    skip_children: &'static str,
    skip_descendants: &'static str,
    skip_advance: &'static str,
}

const WHERE_LABELS: EvaluationLabels = EvaluationLabels {
    undersized_stack: "evaluating an expression with an undersized stack",
    read_node: "evaluating a resolved expression program",
    advance: "advancing through a resolved expression program",
    finish: "finishing a resolved expression program",
    count_children: "counting evaluated expression children",
    skip_node: "short-circuiting a resolved expression program",
    skip_children: "counting skipped expression children",
    skip_descendants: "counting skipped expression descendants",
    skip_advance: "advancing over skipped expression descendants",
};

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
    like_work: LikeWork,
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
    ///
    /// `like_work_limit` bounds the wildcard backtracking of every residual
    /// `LIKE` this evaluator runs, the way the regex backtracking budget bounds
    /// a `LIKE` pushed into a scan pattern. One budget is shared by every row
    /// and every predicate, so moving leaves to the residual program can
    /// neither escape the bound nor multiply it by the rows it visits.
    pub(crate) fn new(program: &Program<'_>, like_work_limit: usize) -> Result<Self> {
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(program.logical_node_count())
            .map_err(|_| Error::Allocation {
                operation: "reserving the expression evaluation stack",
            })?;
        Ok(Self {
            frames,
            like_work: LikeWork::new(like_work_limit),
        })
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
        let Self { frames, like_work } = self;
        run(
            frames,
            program.nodes(),
            program.logical_node_count(),
            WHERE_LABELS,
            |predicate| evaluate_predicate(predicate, rows, like_work),
        )
    }
}

/// Evaluate one flat preorder program with an explicit, preallocated stack.
///
/// `frames` is reused across rows, so evaluation neither recurses nor allocates
/// once the caller has reserved one frame per logical node. Short-circuiting
/// skips whole subtrees rather than evaluating and discarding their leaves.
fn run<Payload>(
    frames: &mut Vec<Frame>,
    nodes: &[Node<Payload>],
    logical_nodes: usize,
    labels: EvaluationLabels,
    mut evaluate_leaf: impl FnMut(&Payload) -> Result<Truth>,
) -> Result<Truth> {
    frames.clear();
    if frames.capacity() < logical_nodes {
        return Err(Error::Capacity {
            operation: labels.undersized_stack,
        });
    }
    let mut cursor = 0_usize;

    loop {
        let node = nodes.get(cursor).ok_or(Error::Capacity {
            operation: labels.read_node,
        })?;
        cursor = cursor.checked_add(1).ok_or(Error::Capacity {
            operation: labels.advance,
        })?;

        let mut value = match node.logical() {
            Some((operator, children)) => {
                frames.push(Frame {
                    operator,
                    remaining: children,
                    value: match operator {
                        LogicalOperator::And => Truth::True,
                        LogicalOperator::Or => Truth::False,
                    },
                });
                continue;
            }
            None => {
                let payload = node.leaf().ok_or(Error::Capacity {
                    operation: labels.read_node,
                })?;
                evaluate_leaf(payload)?
            }
        };

        loop {
            let Some(frame) = frames.last_mut() else {
                if cursor != nodes.len() {
                    return Err(Error::Capacity {
                        operation: labels.finish,
                    });
                }
                return Ok(value);
            };

            frame.remaining = frame.remaining.checked_sub(1).ok_or(Error::Capacity {
                operation: labels.count_children,
            })?;
            frame.value = match frame.operator {
                LogicalOperator::And => frame.value.and(value),
                LogicalOperator::Or => frame.value.or(value),
            };
            let short_circuits = matches!(
                (frame.operator, frame.value),
                (LogicalOperator::And, Truth::False) | (LogicalOperator::Or, Truth::True)
            );
            let skipped = if short_circuits { frame.remaining } else { 0 };
            if skipped > 0 {
                frame.remaining = 0;
                cursor = skip_subtrees(nodes, cursor, skipped, labels)?;
            }

            if frame.remaining > 0 {
                break;
            }
            value = frame.value;
            frames.pop();
        }
    }
}

fn evaluate_predicate(
    predicate: &Predicate<'_>,
    rows: EvaluationRows<'_, '_>,
    like_work: &mut LikeWork,
) -> Result<Truth> {
    let left = value_at(rows, predicate.column())?;
    Ok(match predicate {
        Predicate::Equal { value, .. } => compare_equal(left, value),
        Predicate::NotEqual { value, .. } => match compare_equal(left, value) {
            Truth::True => Truth::False,
            Truth::False => Truth::True,
            Truth::Unknown => Truth::Unknown,
        },
        Predicate::LessThan { value, .. } => compare_ordered(left, value, Ordering::is_lt)?,
        Predicate::LessThanOrEqual { value, .. } => {
            compare_ordered(left, value, |ordering| !ordering.is_gt())?
        }
        Predicate::GreaterThan { value, .. } => compare_ordered(left, value, Ordering::is_gt)?,
        Predicate::GreaterThanOrEqual { value, .. } => {
            compare_ordered(left, value, |ordering| !ordering.is_lt())?
        }
        Predicate::Like { atoms, .. } => match left {
            Value::Text(value) => truth(like::matches_charged(value, atoms, like_work)?),
            Value::Null => Truth::Unknown,
            Value::Integer(_) | Value::Boolean(_) => {
                return Err(Error::Type(String::from(
                    "resolved LIKE predicate was evaluated against a non-TEXT value",
                )));
            }
        },
        Predicate::IsNull { .. } => truth(matches!(left, Value::Null)),
        Predicate::IsNotNull { .. } => truth(!matches!(left, Value::Null)),
        Predicate::In { values, .. } => compare_in(left, values),
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

fn compare_ordered(
    left: &Value,
    right: &Value,
    accepts: impl FnOnce(Ordering) -> bool,
) -> Result<Truth> {
    if matches!(left, Value::Null) {
        return Ok(Truth::Unknown);
    }
    let ordering = match (left, right) {
        (Value::Text(left), Value::Text(right)) => left.chars().cmp(right.chars()),
        (Value::Integer(left), Value::Integer(right)) => left.cmp(right),
        (Value::Boolean(left), Value::Boolean(right)) => left.cmp(right),
        (
            Value::Text(_) | Value::Integer(_) | Value::Boolean(_) | Value::Null,
            Value::Text(_) | Value::Integer(_) | Value::Boolean(_) | Value::Null,
        ) => {
            return Err(Error::Type(String::from(
                "resolved ordered predicate contained incompatible scalar values",
            )));
        }
    };
    Ok(truth(accepts(ordering)))
}

fn compare_in(left: &Value, values: &[Value]) -> Truth {
    if matches!(left, Value::Null) {
        return Truth::Unknown;
    }

    let mut contains_null = false;
    for value in values {
        if matches!(value, Value::Null) {
            contains_null = true;
        } else if values_equal(left, value) {
            return Truth::True;
        }
    }
    if contains_null {
        Truth::Unknown
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

/// Advance `cursor` past `count` whole preorder subtrees.
fn skip_subtrees<Payload>(
    nodes: &[Node<Payload>],
    mut cursor: usize,
    count: usize,
    labels: EvaluationLabels,
) -> Result<usize> {
    let mut pending = count;
    while pending > 0 {
        let node = nodes.get(cursor).ok_or(Error::Capacity {
            operation: labels.skip_node,
        })?;
        pending = pending.checked_sub(1).ok_or(Error::Capacity {
            operation: labels.skip_children,
        })?;
        pending = pending
            .checked_add(node.child_count())
            .ok_or(Error::Capacity {
                operation: labels.skip_descendants,
            })?;
        cursor = cursor.checked_add(1).ok_or(Error::Capacity {
            operation: labels.skip_advance,
        })?;
    }
    Ok(cursor)
}
