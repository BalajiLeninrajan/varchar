//! `DELETE` planning, row removal, and atomic storage commit.

use super::{Database, mutation_plan::MutationPlan};
use crate::query;
use crate::resolve;
use crate::sql::Delete;
use crate::{Outcome, Result};

impl Database {
    pub(super) fn execute_delete(&mut self, statement: Delete) -> Result<Outcome> {
        let table = resolve::require_validated_table(self.storage.catalog(), &statement.table)?;
        let plan = query::compile_scan(table, statement.where_clause.as_ref(), &self.limits)?;
        let mut candidate = self.storage.candidate_with_validation_limits(
            self.limits.max_database_bytes,
            self.limits.max_predicates,
            self.limits.regex_backtrack_limit,
        )?;
        let mutation = MutationPlan::delete(candidate.source(), &plan, &self.limits)?;
        let affected = mutation.apply(&mut candidate)?;
        self.storage = candidate.finish()?;
        Ok(Outcome::Affected { rows: affected })
    }
}
