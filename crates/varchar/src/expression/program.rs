//! Flat resolved Boolean-expression programs.

use crate::Value;
use crate::resolve::ColumnLocation;

use super::like::LikeAtom;
use super::tree::{Leaf, Node, is_well_formed};

/// A semantically validated expression stored in preorder.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Program<'statement> {
    nodes: Vec<ProgramNode<'statement>>,
    logical_nodes: usize,
}

impl<'statement> Program<'statement> {
    pub(crate) fn new(nodes: Vec<ProgramNode<'statement>>) -> Self {
        debug_assert!(is_well_formed(&nodes));
        let logical_nodes = nodes
            .iter()
            .filter(|node| !matches!(node, ProgramNode::Predicate(_)))
            .count();
        Self {
            nodes,
            logical_nodes,
        }
    }

    pub(crate) fn nodes(&self) -> &[ProgramNode<'statement>] {
        &self.nodes
    }

    pub(crate) fn into_nodes(self) -> Vec<ProgramNode<'statement>> {
        self.nodes
    }

    pub(super) const fn logical_node_count(&self) -> usize {
        self.logical_nodes
    }
}

pub(crate) type ProgramNode<'statement> = Node<Predicate<'statement>>;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Predicate<'statement> {
    Equal {
        column: ColumnLocation,
        value: &'statement Value,
    },
    NotEqual {
        column: ColumnLocation,
        value: &'statement Value,
    },
    LessThan {
        column: ColumnLocation,
        value: &'statement Value,
    },
    LessThanOrEqual {
        column: ColumnLocation,
        value: &'statement Value,
    },
    GreaterThan {
        column: ColumnLocation,
        value: &'statement Value,
    },
    GreaterThanOrEqual {
        column: ColumnLocation,
        value: &'statement Value,
    },
    Like {
        column: ColumnLocation,
        atoms: Vec<LikeAtom>,
    },
    IsNull {
        column: ColumnLocation,
    },
    IsNotNull {
        column: ColumnLocation,
    },
    In {
        column: ColumnLocation,
        values: &'statement [Value],
    },
}

impl Leaf for Predicate<'_> {
    fn is_degenerate(&self) -> bool {
        matches!(self, Self::In { values, .. } if values.is_empty())
    }
}

impl Predicate<'_> {
    pub(crate) const fn column(&self) -> ColumnLocation {
        match self {
            Self::Equal { column, .. }
            | Self::NotEqual { column, .. }
            | Self::LessThan { column, .. }
            | Self::LessThanOrEqual { column, .. }
            | Self::GreaterThan { column, .. }
            | Self::GreaterThanOrEqual { column, .. }
            | Self::Like { column, .. }
            | Self::IsNull { column }
            | Self::IsNotNull { column }
            | Self::In { column, .. } => *column,
        }
    }
}
