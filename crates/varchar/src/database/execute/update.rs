//! `UPDATE` planning, row rewriting, and atomic storage commit.

use super::{Database, mutation_plan::MutationPlan};
use crate::query;
use crate::resolve;
use crate::sql::Update;
use crate::{Outcome, Result};

impl Database {
    pub(super) fn execute_update(&mut self, statement: Update) -> Result<Outcome> {
        let table = resolve::require_validated_table(self.storage.catalog(), &statement.table)?;
        let auto_increment = self.storage.catalog().auto_increment(&statement.table);
        let mut assignments =
            resolve::assignments(table.schema(), auto_increment, &statement.assignments)?;
        let plan = query::compile_scan(table, statement.where_clause.as_ref(), &self.limits)?;
        let mut candidate = self.storage.candidate_with_validation_limits(
            self.limits.max_database_bytes,
            self.limits.max_predicates,
            self.limits.regex_backtrack_limit,
        )?;
        if let Some(last) = assignments.next_auto_increment {
            candidate.defer_auto_increment(&statement.table, last)?;
        }
        let mutation =
            MutationPlan::update(&candidate, &plan, &self.limits, &mut assignments.values)?;
        let affected = mutation.apply(&mut candidate)?;
        self.storage = candidate.finish()?;
        Ok(Outcome::Affected { rows: affected })
    }
}
