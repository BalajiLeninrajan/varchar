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
    /// Tables participating in one joined `SELECT`.
    JoinSources,
    /// One generated regular expression.
    GeneratedRegexBytes,
    /// Conservatively accounted decoded-row and join state for `SELECT`.
    QueryWorkingBytes,
    /// A returned [`crate::RowSet`].
    QueryOutputBytes,
    /// Value-comparison work performed by a join.
    JoinSteps,
    /// Backtracking performed by one regex search.
    RegexBacktracking,
}

impl Resource {
    /// A stable machine-readable name for this resource.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DatabaseBytes => "database_bytes",
            Self::SqlBytes => "sql_bytes",
            Self::WherePredicates => "where_predicates",
            Self::JoinSources => "join_sources",
            Self::GeneratedRegexBytes => "generated_regex_bytes",
            Self::QueryWorkingBytes => "query_working_bytes",
            Self::QueryOutputBytes => "query_output_bytes",
            Self::JoinSteps => "join_steps",
            Self::RegexBacktracking => "regex_backtracking",
        }
    }
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
    max_database_bytes: usize,
    max_sql_bytes: usize,
    max_predicates: usize,
    max_join_sources: usize,
    max_pattern_bytes: usize,
    max_query_working_bytes: usize,
    max_query_output_bytes: usize,
    max_join_steps: usize,
    regex_backtrack_limit: usize,
}

impl Limits {
    /// Maximum size of the authoritative database string, in UTF-8 bytes.
    #[must_use]
    pub const fn max_database_bytes(&self) -> usize {
        self.max_database_bytes
    }

    /// Return these limits with a new database-string byte limit.
    #[must_use]
    pub const fn with_max_database_bytes(mut self, limit: usize) -> Self {
        self.max_database_bytes = limit;
        self
    }

    /// Maximum size of one SQL statement, in UTF-8 bytes.
    #[must_use]
    pub const fn max_sql_bytes(&self) -> usize {
        self.max_sql_bytes
    }

    /// Return these limits with a new SQL-statement byte limit.
    #[must_use]
    pub const fn with_max_sql_bytes(mut self, limit: usize) -> Self {
        self.max_sql_bytes = limit;
        self
    }

    /// Maximum number of `WHERE` terms joined by `AND`.
    #[must_use]
    pub const fn max_predicates(&self) -> usize {
        self.max_predicates
    }

    /// Return these limits with a new predicate-count limit.
    #[must_use]
    pub const fn with_max_predicates(mut self, limit: usize) -> Self {
        self.max_predicates = limit;
        self
    }

    /// Maximum number of tables participating in one joined `SELECT`.
    #[must_use]
    pub const fn max_join_sources(&self) -> usize {
        self.max_join_sources
    }

    /// Return these limits with a new joined-source count limit.
    #[must_use]
    pub const fn with_max_join_sources(mut self, limit: usize) -> Self {
        self.max_join_sources = limit;
        self
    }

    /// Maximum size of one generated regular expression, in UTF-8 bytes.
    #[must_use]
    pub const fn max_pattern_bytes(&self) -> usize {
        self.max_pattern_bytes
    }

    /// Return these limits with a new generated-pattern byte limit.
    #[must_use]
    pub const fn with_max_pattern_bytes(mut self, limit: usize) -> Self {
        self.max_pattern_bytes = limit;
        self
    }

    /// Maximum logical `SELECT` working-state charge, in conservatively
    /// accounted bytes.
    ///
    /// This covers transient decoded rows plus decoded source rows and pointer
    /// state retained for joins. It is not a total query or process memory
    /// bound: planning, regex-engine scratch space, the catalog and storage
    /// string, allocator overhead, spare capacity, and mutation candidates are
    /// outside it. `UPDATE` and `DELETE` do not consume this `SELECT` budget.
    /// Returned results have a separate logical byte limit.
    #[must_use]
    pub const fn max_query_working_bytes(&self) -> usize {
        self.max_query_working_bytes
    }

    /// Return these limits with a new logical `SELECT` working-state limit.
    #[must_use]
    pub const fn with_max_query_working_bytes(mut self, limit: usize) -> Self {
        self.max_query_working_bytes = limit;
        self
    }

    /// Maximum returned query-output size, in conservatively accounted bytes.
    #[must_use]
    pub const fn max_query_output_bytes(&self) -> usize {
        self.max_query_output_bytes
    }

    /// Return these limits with a new query-output byte limit.
    #[must_use]
    pub const fn with_max_query_output_bytes(mut self, limit: usize) -> Self {
        self.max_query_output_bytes = limit;
        self
    }

    /// Maximum amount of value-comparison work performed while joining rows.
    #[must_use]
    pub const fn max_join_steps(&self) -> usize {
        self.max_join_steps
    }

    /// Return these limits with a new join-work limit.
    #[must_use]
    pub const fn with_max_join_steps(mut self, limit: usize) -> Self {
        self.max_join_steps = limit;
        self
    }

    /// Per-search backtracking limit passed to the regex engine.
    #[must_use]
    pub const fn regex_backtrack_limit(&self) -> usize {
        self.regex_backtrack_limit
    }

    /// Return these limits with a new regex backtracking limit.
    #[must_use]
    pub const fn with_regex_backtrack_limit(mut self, limit: usize) -> Self {
        self.regex_backtrack_limit = limit;
        self
    }
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
mod tests {
    use super::{Limits, Resource};

    #[test]
    fn builders_change_only_the_selected_limits() {
        let defaults = Limits::default();
        let limits = defaults
            .clone()
            .with_max_database_bytes(1)
            .with_max_sql_bytes(2)
            .with_max_predicates(3)
            .with_max_join_sources(4)
            .with_max_pattern_bytes(5)
            .with_max_query_working_bytes(6)
            .with_max_query_output_bytes(7)
            .with_max_join_steps(8)
            .with_regex_backtrack_limit(9);

        assert_eq!(limits.max_database_bytes(), 1);
        assert_eq!(limits.max_sql_bytes(), 2);
        assert_eq!(limits.max_predicates(), 3);
        assert_eq!(limits.max_join_sources(), 4);
        assert_eq!(limits.max_pattern_bytes(), 5);
        assert_eq!(limits.max_query_working_bytes(), 6);
        assert_eq!(limits.max_query_output_bytes(), 7);
        assert_eq!(limits.max_join_steps(), 8);
        assert_eq!(limits.regex_backtrack_limit(), 9);
        assert_eq!(Limits::default(), defaults);
    }

    #[test]
    fn resources_expose_stable_names_and_readable_displays() {
        let cases = [
            (Resource::DatabaseBytes, "database_bytes", "database bytes"),
            (Resource::SqlBytes, "sql_bytes", "SQL bytes"),
            (
                Resource::WherePredicates,
                "where_predicates",
                "WHERE predicates",
            ),
            (Resource::JoinSources, "join_sources", "JOIN sources"),
            (
                Resource::GeneratedRegexBytes,
                "generated_regex_bytes",
                "generated regex bytes",
            ),
            (
                Resource::QueryWorkingBytes,
                "query_working_bytes",
                "query working bytes",
            ),
            (
                Resource::QueryOutputBytes,
                "query_output_bytes",
                "query output bytes",
            ),
            (Resource::JoinSteps, "join_steps", "JOIN execution steps"),
            (
                Resource::RegexBacktracking,
                "regex_backtracking",
                "regex backtracking steps",
            ),
        ];

        for (resource, machine_name, human_display) in cases {
            assert_eq!(resource.as_str(), machine_name);
            assert_eq!(resource.to_string(), human_display);
        }
    }
}
