//! Resource policy shared by request validation, resolution, and execution.

use crate::{Error, Result};

const MIB: usize = 1024 * 1024;

/// Resource bounds applied by the platform-neutral database core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Maximum size of the authoritative database string, in UTF-8 bytes.
    pub max_database_bytes: usize,
    /// Maximum size of one SQL statement, in UTF-8 bytes.
    pub max_sql_bytes: usize,
    /// Maximum number of `WHERE` terms joined by `AND`.
    pub max_predicates: usize,
    /// Maximum size of a generated regular expression, in UTF-8 bytes.
    pub max_pattern_bytes: usize,
    /// Maximum amount of typed value data materialized by a query.
    pub max_result_bytes: usize,
    /// Per-search backtracking limit passed to the regex engine.
    pub regex_backtrack_limit: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_database_bytes: 64 * MIB,
            max_sql_bytes: 64 * 1024,
            max_predicates: 64,
            max_pattern_bytes: 8 * MIB,
            max_result_bytes: 64 * MIB,
            regex_backtrack_limit: 1_000_000,
        }
    }
}

pub(crate) fn check_limit(actual: usize, limit: usize, resource: &'static str) -> Result<()> {
    if actual > limit {
        Err(Error::ResourceLimit { resource, limit })
    } else {
        Ok(())
    }
}
