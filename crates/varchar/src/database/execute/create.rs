//! `CREATE TABLE` resolution and atomic storage commit.

use super::Database;
use crate::resolve;
use crate::sql::CreateTable;
use crate::{Outcome, Result};

impl Database {
    pub(super) fn execute_create(&mut self, statement: CreateTable) -> Result<Outcome> {
        let resolved = resolve::create_schema_with_limit(
            self.storage.catalog(),
            statement,
            self.limits.max_predicates,
        )?;
        let table = resolved.schema.name.clone();
        let mut candidate = self.storage.candidate_with_validation_limits(
            self.limits.max_database_bytes,
            self.limits.max_predicates,
        )?;
        candidate.insert_schema_with_auto_increment(&resolved.schema, resolved.auto_increment)?;
        self.storage = candidate.finish()?;
        Ok(Outcome::Created { table })
    }
}
