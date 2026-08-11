//! Schema-aware SQL name and type resolution.
//!
//! This layer turns parser-owned names into column positions and validates
//! logical values. It deliberately knows nothing about storage encodings,
//! regular expressions, row scans, or candidate commits.

mod column;
mod create;
mod expression;
mod insert;
mod join;
mod order;
mod projection;
mod select;
mod source;
mod table;
mod update;

pub(crate) use crate::expression::{LikeAtom, Predicate as ResolvedPredicate};
pub(crate) use column::ColumnLocation;
#[cfg(test)]
pub(crate) use create::create_schema;
pub(crate) use create::create_schema_with_limit;
pub(crate) use expression::local_expression;
#[cfg(test)]
pub(crate) use expression::predicate;
pub(crate) use insert::insert_values;
pub(crate) use join::{ResolvedJoin, ResolvedJoinCondition};
pub(crate) use order::ResolvedOrderTerm;
pub(crate) use select::{ResolvedSelect, select};
pub(crate) use table::require_table;
pub(crate) use update::assignments;

#[cfg(test)]
mod tests;
