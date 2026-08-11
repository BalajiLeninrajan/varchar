//! `UPDATE` planning, row rewriting, and atomic storage commit.

use super::Database;
use crate::query;
use crate::resolve;
use crate::sql::Update;
use crate::{Outcome, Result};

impl Database {
    pub(super) fn execute_update(&mut self, statement: Update) -> Result<Outcome> {
        let schema = resolve::require_table(self.storage.catalog(), &statement.table)?;
        let auto_increment = self.storage.catalog().auto_increment(&statement.table);
        let assignments = resolve::assignments(schema, auto_increment, &statement.assignments)?;
        let plan = query::compile_scan(schema, statement.where_clause.as_ref(), &self.limits)?;
        let mut candidate = self.storage.candidate_with_validation_limits(
            self.limits.max_database_bytes,
            self.limits.max_predicates,
            self.limits.regex_backtrack_limit,
        )?;
        if let Some(last) = assignments.next_auto_increment {
            candidate.defer_auto_increment(&statement.table, last)?;
        }
        let affected =
            query::rewrite_matching_rows(&mut candidate, &plan, &self.limits, |mut values| {
                for (index, value) in &assignments.values {
                    values[*index] = value.clone();
                }
                Ok(Some(values))
            })?;
        self.storage = candidate.finish()?;
        Ok(Outcome::Affected { rows: affected })
    }
}
