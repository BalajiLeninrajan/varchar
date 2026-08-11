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

/// The structural rules one flat program is validated against.
#[derive(Clone, Copy)]
pub(crate) struct ShapeRules {
    reject_associative_nesting: bool,
}

impl ShapeRules {
    /// Every program must be one complete preorder tree whose logical nodes
    /// each have at least two children and whose leaves are all satisfiable.
    pub(crate) const COMPLETE: Self = Self {
        reject_associative_nesting: false,
    };

    /// Persisted `CHECK` programs additionally store associative chains
    /// flattened, so an `AND` directly beneath an `AND` — or an `OR` beneath an
    /// `OR` — is a second encoding of a tree that already has a canonical one.
    pub(crate) const CANONICAL: Self = Self {
        reject_associative_nesting: true,
    };
}

/// The operation labels a shape walk reports its failures under.
///
/// Each pipeline keeps its own wording rather than the shared walk inventing a
/// neutral one, so unifying the walk leaves every diagnostic unchanged.
#[derive(Clone, Copy)]
pub(crate) struct ShapeLabels {
    /// Arithmetic that cannot represent the program's own child counts.
    pub(crate) capacity: &'static str,
    /// A failure to reserve one more open logical frame.
    pub(crate) allocation: &'static str,
}

impl ShapeLabels {
    /// Labels for the debug assertions that guard program construction, which
    /// discard the error and report only that the program is malformed.
    const CONSTRUCTION: Self = Self {
        capacity: "validating a constructed expression program shape",
        allocation: "reserving constructed expression shape validation state",
    };
}

/// One logical node whose children are still being read.
struct Frame {
    operator: LogicalOperator,
    remaining: usize,
}

/// Byte accounting for the frames a canonical-shape walk keeps open.
pub(crate) trait FrameAccounting {
    fn retain(&mut self, frames: usize) -> Result<()>;
    fn release(&mut self, frames: usize);
}

/// Accounting for callers that do not meter shape-validation state.
pub(crate) struct UntrackedFrames;

impl FrameAccounting for UntrackedFrames {
    fn retain(&mut self, _frames: usize) -> Result<()> {
        Ok(())
    }

    fn release(&mut self, _frames: usize) {}
}

/// The byte size of one simultaneously open logical frame.
#[cfg(test)]
pub(crate) const fn frame_bytes() -> usize {
    std::mem::size_of::<Frame>()
}

/// Whether `nodes` satisfy `rules`.
///
/// Reserved for the debug assertions that guard program construction: they have
/// no error channel, so an exhausted allocator reads as a malformed program.
pub(crate) fn is_well_formed<Payload: Leaf>(nodes: &[Node<Payload>], rules: ShapeRules) -> bool {
    validate_shape(
        nodes,
        rules,
        &mut UntrackedFrames,
        ShapeLabels::CONSTRUCTION,
    )
    .unwrap_or(false)
}

/// Whether `nodes` form one complete preorder tree obeying `rules`.
///
/// `Ok(false)` reports a malformed program; the error channel is reserved for a
/// walk that could not be carried out at all, so callers that turn a malformed
/// program into a typed error keep that error distinct from exhaustion.
///
/// Only the associative-nesting rule needs to remember ancestors, so a walk
/// without it neither allocates nor charges `accounting`.
pub(crate) fn validate_shape<Payload: Leaf>(
    nodes: &[Node<Payload>],
    rules: ShapeRules,
    accounting: &mut impl FrameAccounting,
    labels: ShapeLabels,
) -> Result<bool> {
    let mut open = Vec::new();
    let result = walk_shape(nodes, rules, accounting, labels, &mut open);
    while !open.is_empty() {
        release_frame(&mut open, accounting);
    }
    result
}

fn walk_shape<Payload: Leaf>(
    nodes: &[Node<Payload>],
    rules: ShapeRules,
    accounting: &mut impl FrameAccounting,
    labels: ShapeLabels,
    open: &mut Vec<Frame>,
) -> Result<bool> {
    let mut pending = 1_usize;

    for node in nodes {
        let Some(after_node) = pending.checked_sub(1) else {
            return Ok(false);
        };
        pending = after_node;

        if rules.reject_associative_nesting
            && let Some(parent) = open.last_mut()
        {
            let Some(remaining) = parent.remaining.checked_sub(1) else {
                return Ok(false);
            };
            parent.remaining = remaining;
        }

        let children = match node.logical() {
            Some((operator, children)) => {
                if children < 2 {
                    return Ok(false);
                }
                if rules.reject_associative_nesting {
                    if open
                        .last()
                        .is_some_and(|parent| parent.operator == operator)
                    {
                        return Ok(false);
                    }
                    retain_frame(open, accounting, labels)?;
                    open.push(Frame {
                        operator,
                        remaining: children,
                    });
                }
                children
            }
            None => {
                if node.leaf().is_some_and(Leaf::is_degenerate) {
                    return Ok(false);
                }
                if rules.reject_associative_nesting {
                    while open.last().is_some_and(|frame| frame.remaining == 0) {
                        release_frame(open, accounting);
                    }
                }
                0
            }
        };

        pending = pending.checked_add(children).ok_or(Error::Capacity {
            operation: labels.capacity,
        })?;
    }

    Ok(pending == 0)
}

fn retain_frame(
    open: &mut Vec<Frame>,
    accounting: &mut impl FrameAccounting,
    labels: ShapeLabels,
) -> Result<()> {
    accounting.retain(1)?;
    if open.try_reserve(1).is_err() {
        accounting.release(1);
        return Err(Error::Allocation {
            operation: labels.allocation,
        });
    }
    Ok(())
}

fn release_frame(open: &mut Vec<Frame>, accounting: &mut impl FrameAccounting) {
    open.pop()
        .expect("shape accounting only releases retained frames");
    accounting.release(1);
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
