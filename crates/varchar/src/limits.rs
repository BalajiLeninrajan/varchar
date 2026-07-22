//! Resource policy shared by request validation, resolution, and execution.

use std::fmt;

use crate::{Error, Result};

const MIB: usize = 1024 * 1024;

/// A configurable resource governed by [`Limits`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Resource {
    /// The authoritative database string.
    DatabaseBytes,
    /// One SQL statement.
    SqlBytes,
    /// `WHERE` predicates joined by `AND`.
    WherePredicates,
    /// Source tables participating in one `SELECT`.
    JoinSources,
    /// One generated regular expression.
    GeneratedRegexBytes,
    /// Conservatively accounted decoded-row state for `SELECT`.
    QueryWorkingBytes,
    /// Returned query data, explanations, and resolved-projection planning.
    QueryOutputBytes,
    /// Value-comparison work performed by a join.
    JoinSteps,
    /// Backtracking performed by one regex search.
    RegexBacktracking,
}

impl fmt::Display for Resource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DatabaseBytes => "database bytes",
            Self::SqlBytes => "SQL bytes",
            Self::WherePredicates => "WHERE predicates",
            Self::JoinSources => "JOIN sources",
            Self::GeneratedRegexBytes => "generated regex bytes",
            Self::QueryWorkingBytes => "query working bytes",
            Self::QueryOutputBytes => "query output bytes",
            Self::JoinSteps => "JOIN execution steps",
            Self::RegexBacktracking => "regex backtracking steps",
        })
    }
}

/// Resource bounds applied by the platform-neutral database core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Maximum size of the authoritative database string, in UTF-8 bytes.
    pub max_database_bytes: usize,
    /// Maximum size of one SQL statement, in UTF-8 bytes.
    pub max_sql_bytes: usize,
    /// Maximum number of `WHERE` terms joined by `AND`.
    pub max_predicates: usize,
    /// Maximum number of source tables participating in one `SELECT`.
    pub max_join_sources: usize,
    /// Maximum size of one generated regular expression, in UTF-8 bytes.
    ///
    /// The regex compiler also receives this value as an approximate compiled
    /// delegate safeguard. Refusals from that safeguard remain
    /// [`crate::Error::RegexCompile`] errors; [`Resource::GeneratedRegexBytes`]
    /// describes generated pattern text, not compiled engine state.
    pub max_pattern_bytes: usize,
    /// Maximum logical `SELECT` working-state charge, in conservatively
    /// accounted bytes.
    ///
    /// A single-table query retains one decoded row at a time, so its charge is
    /// a peak-per-row budget rather than a cumulative scan budget. A joined
    /// query cumulatively charges decoded source rows and its chosen-row pointer
    /// stack. This does not govern `UPDATE` or `DELETE`; returned rows have a
    /// separate limit. Charges include target-layout sizes, so an exact boundary
    /// can differ between 32-bit and 64-bit builds.
    pub max_query_working_bytes: usize,
    /// Maximum conservatively accounted bytes for one materialized `SELECT`
    /// result or `SELECT` explanation.
    ///
    /// The resolver also uses this value as an independent bound on the
    /// expanded projection-location plan before query compilation.
    ///
    /// Charges include target-layout sizes, so an exact boundary can differ
    /// between 32-bit and 64-bit builds.
    pub max_query_output_bytes: usize,
    /// Maximum amount of value-comparison work performed while joining rows.
    pub max_join_steps: usize,
    /// Per-search backtracking limit passed to the regex engine.
    pub regex_backtrack_limit: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_database_bytes: 64 * MIB,
            max_sql_bytes: 64 * 1024,
            max_predicates: 64,
            max_join_sources: 64,
            max_pattern_bytes: 8 * MIB,
            max_query_working_bytes: 32 * MIB,
            max_query_output_bytes: 32 * MIB,
            max_join_steps: 1_000_000,
            regex_backtrack_limit: 1_000_000,
        }
    }
}

pub(crate) fn check_limit(actual: usize, limit: usize, resource: Resource) -> Result<()> {
    if actual > limit {
        Err(Error::ResourceLimit { resource, limit })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
