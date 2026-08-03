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
            let units = match node {
                ExpressionNode::Predicate(Predicate {
                    operator: PredicateOperator::In(values),
                    ..
                }) => values.len(),
                ExpressionNode::Predicate(_) => 1,
                ExpressionNode::And { .. } | ExpressionNode::Or { .. } => 0,
            };
            count.checked_add(units).ok_or(Error::Capacity {
                operation: "counting WHERE predicates",
            })
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
    In(Vec<Value>),
}

fn valid_program(nodes: &[ExpressionNode]) -> bool {
    let mut pending = 1_usize;
    for node in nodes {
        let Some(after_node) = pending.checked_sub(1) else {
            return false;
        };
        pending = after_node;

        if matches!(
            node,
            ExpressionNode::Predicate(Predicate {
                operator: PredicateOperator::In(values),
                ..
            }) if values.is_empty()
        ) {
            return false;
        }
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
