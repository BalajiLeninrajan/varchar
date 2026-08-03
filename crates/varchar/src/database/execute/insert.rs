//! `INSERT` resolution and atomic storage commit.

use super::Database;
use crate::resolve;
use crate::sql::Insert;
use crate::{Outcome, Result};

impl Database {
    pub(super) fn execute_insert(&mut self, statement: Insert) -> Result<Outcome> {
        let schema = resolve::require_table(self.storage.catalog(), &statement.table)?;
        let auto_increment = self.storage.catalog().auto_increment(&statement.table);
        let resolved =
            resolve::insert_values(schema, auto_increment, statement.columns, statement.values)?;
        let mut candidate = self.storage.candidate_with_validation_limits(
            self.limits.max_database_bytes,
            self.limits.max_predicates,
            self.limits.regex_backtrack_limit,
        )?;
        if let Some(last) = resolved.next_auto_increment {
            candidate.advance_auto_increment(&statement.table, last)?;
        }
        candidate.append_row(schema.row_layout(), &resolved.values)?;
        self.storage = candidate.finish()?;
        Ok(Outcome::Affected { rows: 1 })
    }
}
