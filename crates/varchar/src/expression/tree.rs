//! The one flat preorder Boolean-expression program shared by every pipeline.
//!
//! Parsed `WHERE` clauses, resolved `WHERE` programs, and owned `CHECK`
//! programs differ only in the payload each leaf carries, so they share one
//! node type, one shape validator, and one set of preorder walks. A new node
//! kind or structural invariant is therefore added exactly once.

use crate::{Error, Result};

/// The two associative connectives a program nests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LogicalOperator {
    And,
    Or,
}

/// One node of a flat preorder program carrying `Payload` at its leaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Node<Payload> {
    And { children: usize },
    Or { children: usize },
    Predicate(Payload),
}

impl<Payload> Node<Payload> {
    /// How many immediate children follow the node in preorder.
    pub(crate) const fn child_count(&self) -> usize {
        match self {
            Self::And { children } | Self::Or { children } => *children,
            Self::Predicate(_) => 0,
        }
    }

    /// The connective and child count of a logical node.
    pub(crate) const fn logical(&self) -> Option<(LogicalOperator, usize)> {
        match self {
            Self::And { children } => Some((LogicalOperator::And, *children)),
            Self::Or { children } => Some((LogicalOperator::Or, *children)),
            Self::Predicate(_) => None,
        }
    }

    /// The payload of a leaf node.
    pub(crate) const fn leaf(&self) -> Option<&Payload> {
        match self {
            Self::And { .. } | Self::Or { .. } => None,
            Self::Predicate(payload) => Some(payload),
        }
    }
}

/// A leaf payload of a flat program.
pub(crate) trait Leaf {
    /// Whether the leaf can never appear in a well-formed program.
    ///
    /// `IN ()` is the only such leaf: an empty item list has no truth value, so
    /// every stage rejects it instead of evaluating it.
    fn is_degenerate(&self) -> bool;
}

/// Whether `nodes` form one complete preorder tree.
///
/// Reserved for the debug assertions that guard program construction: every
/// logical node needs at least two children, every leaf has to be satisfiable,
/// and the walk has to consume the program exactly.
pub(crate) fn is_well_formed<Payload: Leaf>(nodes: &[Node<Payload>]) -> bool {
    walk_shape(nodes)
}

fn walk_shape<Payload: Leaf>(nodes: &[Node<Payload>]) -> bool {
    let mut pending = 1_usize;

    for node in nodes {
        let Some(after_node) = pending.checked_sub(1) else {
            return false;
        };
        pending = after_node;

        let children = match node.logical() {
            Some((_, children)) => {
                if children < 2 {
                    return false;
                }
                children
            }
            None => {
                if node.leaf().is_some_and(Leaf::is_degenerate) {
                    return false;
                }
                0
            }
        };

        let Some(next) = pending.checked_add(children) else {
            return false;
        };
        pending = next;
    }

    pending == 0
}

/// The size of the subtree rooted at each node, in preorder positions.
pub(crate) fn subtree_sizes<Payload>(nodes: &[Node<Payload>]) -> Result<Vec<usize>> {
    let mut sizes = Vec::new();
    sizes
        .try_reserve_exact(nodes.len())
        .map_err(|_| Error::Allocation {
            operation: "reserving expression subtree sizes",
        })?;
    sizes.resize(nodes.len(), 0);

    let mut pending = Vec::new();
    pending
        .try_reserve_exact(nodes.len())
        .map_err(|_| Error::Allocation {
            operation: "reserving expression subtree measurements",
        })?;

    for (index, node) in nodes.iter().enumerate().rev() {
        let mut size = 1_usize;
        let children = node.child_count();
        if pending.len() < children {
            return Err(malformed_subtree());
        }
        for _ in 0..children {
            size = size
                .checked_add(pending.pop().ok_or_else(malformed_subtree)?)
                .ok_or(Error::Capacity {
                    operation: "measuring an expression subtree",
                })?;
        }
        sizes[index] = size;
        pending.push(size);
    }
    if pending.len() != 1 {
        return Err(malformed_subtree());
    }
    Ok(sizes)
}

fn malformed_subtree() -> Error {
    Error::Capacity {
        operation: "measuring an incomplete expression program",
    }
}
