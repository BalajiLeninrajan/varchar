//! Flat parsed Boolean-expression programs.

use crate::{Error, Result, Value};

use super::ColumnRef;

/// One normalized expression stored in preorder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Expression {
    nodes: Vec<ExpressionNode>,
}

impl Expression {
    pub(crate) fn new(nodes: Vec<ExpressionNode>) -> Self {
        debug_assert!(valid_program(&nodes));
        Self { nodes }
    }

    pub(crate) fn nodes(&self) -> &[ExpressionNode] {
        &self.nodes
    }

    pub(crate) fn predicate_units(&self) -> Result<usize> {
        self.nodes.iter().try_fold(0_usize, |count, node| {
            if matches!(node, ExpressionNode::Predicate(_)) {
                count.checked_add(1).ok_or(Error::Capacity {
                    operation: "counting WHERE predicates",
                })
            } else {
                Ok(count)
            }
        })
    }
}

/// One node in a normalized preorder expression program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExpressionNode {
    And { children: usize },
    Or { children: usize },
    Predicate(Predicate),
}

impl ExpressionNode {
    pub(crate) const fn child_count(&self) -> usize {
        match self {
            Self::And { children } | Self::Or { children } => *children,
            Self::Predicate(_) => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Predicate {
    pub(crate) column: ColumnRef,
    pub(crate) operator: PredicateOperator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PredicateOperator {
    Equal(Value),
    NotEqual(Value),
    LessThan(Value),
    LessThanOrEqual(Value),
    GreaterThan(Value),
    GreaterThanOrEqual(Value),
    Like(String),
    IsNull,
    IsNotNull,
}

fn valid_program(nodes: &[ExpressionNode]) -> bool {
    let mut pending = 1_usize;
    for node in nodes {
        let Some(after_node) = pending.checked_sub(1) else {
            return false;
        };
        pending = after_node;

        let children = node.child_count();
        if matches!(node, ExpressionNode::And { .. } | ExpressionNode::Or { .. }) && children < 2 {
            return false;
        }
        let Some(next) = pending.checked_add(children) else {
            return false;
        };
        pending = next;
    }
    pending == 0
}
