//! Flat resolved Boolean-expression programs.

use crate::resolve::ColumnLocation;
use crate::{Error, Result, Value};

use super::like::LikeAtom;
use super::tree::{
    FrameAccounting, Leaf, Node, ShapeLabels, ShapeRules, UntrackedFrames, is_well_formed,
    validate_shape,
};

/// The shape errors a `CHECK` program reports when it is not canonical.
const CHECK_SHAPE_LABELS: ShapeLabels = ShapeLabels {
    capacity: "validating CHECK program shape",
    allocation: "reserving CHECK shape validation state",
};

/// A semantically validated expression stored in preorder.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Program<'statement> {
    nodes: Vec<ProgramNode<'statement>>,
    logical_nodes: usize,
}

impl<'statement> Program<'statement> {
    pub(crate) fn new(nodes: Vec<ProgramNode<'statement>>) -> Self {
        debug_assert!(is_well_formed(&nodes, ShapeRules::COMPLETE));
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

/// One owned, resolved CHECK expression stored in flat preorder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CheckProgram {
    nodes: Vec<CheckProgramNode>,
    logical_nodes: usize,
}

impl CheckProgram {
    pub(crate) fn new(nodes: Vec<CheckProgramNode>) -> Self {
        debug_assert!(is_well_formed(&nodes, ShapeRules::COMPLETE));
        let logical_nodes = nodes
            .iter()
            .filter(|node| !matches!(node, CheckProgramNode::Predicate(_)))
            .count();
        Self {
            nodes,
            logical_nodes,
        }
    }

    pub(crate) fn nodes(&self) -> &[CheckProgramNode] {
        &self.nodes
    }

    pub(crate) const fn logical_node_count(&self) -> usize {
        self.logical_nodes
    }

    /// Reject any program that is not the canonical encoding of its tree.
    pub(crate) fn validate_shape(&self) -> Result<()> {
        self.validate_shape_with_accounting(&mut UntrackedFrames)
    }

    fn validate_shape_with_accounting(&self, accounting: &mut impl FrameAccounting) -> Result<()> {
        if validate_shape(
            &self.nodes,
            ShapeRules::CANONICAL,
            accounting,
            CHECK_SHAPE_LABELS,
        )? {
            Ok(())
        } else {
            Err(Error::Schema(String::from(
                "CHECK program is not a canonical complete expression",
            )))
        }
    }

    #[cfg(test)]
    pub(super) fn validate_shape_with_budget(&self, budget: &mut LogicalFrameBudget) -> Result<()> {
        self.validate_shape_with_accounting(budget)
    }
}

pub(crate) type CheckProgramNode = Node<CheckPredicate>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CheckPredicate {
    Equal { column: usize, value: Value },
    NotEqual { column: usize, value: Value },
    LessThan { column: usize, value: Value },
    LessThanOrEqual { column: usize, value: Value },
    GreaterThan { column: usize, value: Value },
    GreaterThanOrEqual { column: usize, value: Value },
    Like { column: usize, atoms: Vec<LikeAtom> },
    IsNull { column: usize },
    IsNotNull { column: usize },
    In { column: usize, values: Vec<Value> },
}

impl Leaf for CheckPredicate {
    fn is_degenerate(&self) -> bool {
        matches!(self, Self::In { values, .. } if values.is_empty())
    }
}

impl CheckPredicate {
    pub(crate) const fn column(&self) -> usize {
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

#[cfg(test)]
pub(super) struct LogicalFrameBudget {
    limit: usize,
    used: usize,
    peak: usize,
}

#[cfg(test)]
impl LogicalFrameBudget {
    pub(super) const fn new(limit: usize) -> Self {
        Self {
            limit,
            used: 0,
            peak: 0,
        }
    }

    pub(super) const fn frame_bytes() -> usize {
        super::tree::frame_bytes()
    }

    pub(super) const fn used(&self) -> usize {
        self.used
    }

    pub(super) const fn peak(&self) -> usize {
        self.peak
    }

    const fn error(&self) -> Error {
        Error::ResourceLimit {
            resource: crate::Resource::StorageWorkingBytes,
            limit: self.limit,
        }
    }
}

#[cfg(test)]
impl FrameAccounting for LogicalFrameBudget {
    fn retain(&mut self, frames: usize) -> Result<()> {
        let bytes = frames
            .checked_mul(Self::frame_bytes())
            .ok_or_else(|| self.error())?;
        let used = self.used.checked_add(bytes).ok_or_else(|| self.error())?;
        if used > self.limit {
            return Err(self.error());
        }
        self.used = used;
        self.peak = self.peak.max(used);
        Ok(())
    }

    fn release(&mut self, frames: usize) {
        let bytes = frames
            .checked_mul(Self::frame_bytes())
            .expect("retained CHECK frame bytes were previously representable");
        debug_assert!(bytes <= self.used);
        self.used -= bytes;
    }
}
