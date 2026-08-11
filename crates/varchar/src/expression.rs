//! Flat Boolean-expression programs, validation products, and evaluation.

mod evaluate;
mod format;
mod like;
mod program;
mod truth;

pub(crate) use evaluate::Evaluator;
pub(crate) use like::{LikeAtom, compile_pattern};
pub(crate) use program::{Predicate, Program, ProgramNode};

#[cfg(test)]
mod tests;
