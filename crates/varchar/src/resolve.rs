//! Schema-aware SQL name and type resolution.
//!
//! This layer turns parser-owned names into column positions and validates
//! logical values. It deliberately knows nothing about storage encodings,
//! regular expressions, row scans, or candidate commits.

mod column;
mod create;
mod insert;
mod predicate;
mod projection;
mod table;
mod update;

pub(crate) use create::create_schema;
pub(crate) use insert::insert_values;
pub(crate) use predicate::{ResolvedPredicate, predicate};
pub(crate) use projection::projection;
pub(crate) use table::require_table;
pub(crate) use update::assignments;

#[cfg(test)]
mod tests;
