//! Flat Boolean-expression programs, validation products, and evaluation.

mod evaluate;
mod format;
mod like;
mod program;
mod tree;
mod truth;

pub(crate) use evaluate::Evaluator;
pub(crate) use like::{LikeAtom, compile_pattern};
pub(crate) use program::{Predicate, Program, ProgramNode};
pub(crate) use tree::{Leaf, Node, is_well_formed, subtree_sizes};

#[cfg(test)]
mod tests;
