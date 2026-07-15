//! Public engine-produced plans and statement results.

mod column;
mod outcome;
mod row_set;
mod select_explanation;

pub use column::{ColumnOrigin, ResultColumn};
pub use outcome::Outcome;
pub use row_set::RowSet;
pub use select_explanation::SelectExplanation;

#[cfg(test)]
mod tests;
