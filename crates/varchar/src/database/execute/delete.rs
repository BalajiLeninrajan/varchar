//! `DELETE` planning, row removal, and atomic storage commit.

use super::Database;
use crate::query;
use crate::resolve;
use crate::sql::Delete;
use crate::{Outcome, Result};

impl Database {
    pub(super) fn execute_delete(&mut self, statement: Delete) -> Result<Outcome> {
        let schema = resolve::require_table(self.storage.catalog(), &statement.table)?;
        let plan = query::compile_scan(schema, statement.where_clause.as_ref(), &self.limits)?;
        let mut candidate = self.storage.candidate(self.limits.max_database_bytes)?;
        let affected =
            query::rewrite_matching_rows(&mut candidate, &plan, &self.limits, |_| Ok(None))?;
        self.storage = candidate.finish()?;
        Ok(Outcome::Affected { rows: affected })
    }
}
