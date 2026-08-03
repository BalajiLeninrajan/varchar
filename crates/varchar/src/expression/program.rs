//! Flat resolved Boolean-expression programs.

use crate::Value;
use crate::resolve::ColumnLocation;

use super::like::LikeAtom;

/// A semantically validated expression stored in preorder.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Program<'statement> {
    nodes: Vec<ProgramNode<'statement>>,
    logical_nodes: usize,
}

impl<'statement> Program<'statement> {
    pub(crate) fn new(nodes: Vec<ProgramNode<'statement>>) -> Self {
        debug_assert!(valid_program(&nodes));
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

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProgramNode<'statement> {
    And { children: usize },
    Or { children: usize },
    Predicate(Predicate<'statement>),
}

impl ProgramNode<'_> {
    pub(crate) const fn child_count(&self) -> usize {
        match self {
            Self::And { children } | Self::Or { children } => *children,
            Self::Predicate(_) => 0,
        }
    }
}

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
}

impl Predicate<'_> {
    pub(crate) const fn column(&self) -> ColumnLocation {
        match self {
            Self::Equal { column, .. }
            | Self::NotEqual { column, .. }
            | Self::Like { column, .. }
            | Self::IsNull { column }
            | Self::IsNotNull { column } => *column,
        }
    }
}

fn valid_program(nodes: &[ProgramNode<'_>]) -> bool {
    let mut pending = 1_usize;
    for node in nodes {
        let Some(after_node) = pending.checked_sub(1) else {
            return false;
        };
        pending = after_node;

        let children = node.child_count();
        if matches!(node, ProgramNode::And { .. } | ProgramNode::Or { .. }) && children < 2 {
            return false;
        }
        let Some(next) = pending.checked_add(children) else {
            return false;
        };
        pending = next;
    }
    pending == 0
}
