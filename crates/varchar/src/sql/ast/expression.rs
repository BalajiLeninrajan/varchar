//! Flat parsed Boolean-expression programs.

use crate::expression::{Leaf, Node, ShapeRules, is_well_formed};
use crate::{Error, Result, Value};

use super::ColumnRef;

/// One normalized expression stored in preorder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Expression {
    nodes: Vec<ExpressionNode>,
}

impl Expression {
    pub(crate) fn new(nodes: Vec<ExpressionNode>) -> Self {
        debug_assert!(is_well_formed(&nodes, ShapeRules::COMPLETE));
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
                operation: "counting expression predicates",
            })
        })
    }
}

/// One node in a normalized preorder expression program.
pub(crate) type ExpressionNode = Node<Predicate>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Predicate {
    pub(crate) column: ColumnRef,
    pub(crate) operator: PredicateOperator,
}

impl Leaf for Predicate {
    fn is_degenerate(&self) -> bool {
        matches!(&self.operator, PredicateOperator::In(values) if values.is_empty())
    }
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
