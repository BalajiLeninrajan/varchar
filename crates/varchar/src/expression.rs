//! Flat Boolean-expression programs, validation products, and evaluation.

mod like;
mod program;

pub(crate) use like::{LikeAtom, compile_pattern};
pub(crate) use program::Predicate;
