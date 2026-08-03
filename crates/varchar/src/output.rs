//! Public engine-produced plans and statement results.

mod column;
mod outcome;
mod row_set;
mod row_set_builder;
mod select_explanation;

pub use column::{ColumnOrigin, ResultColumn};
pub use outcome::Outcome;
pub use row_set::RowSet;
pub(crate) use row_set_builder::{ResultCell, ResultColumnSpec, RowSetBuilder};
pub use select_explanation::SelectExplanation;

#[cfg(test)]
mod tests;
