//! Request validation, statement dispatch, and query-plan delegation.

mod create;
mod delete;
mod insert;
mod update;

use super::Database;
use crate::limits::check_limit;
use crate::query::{self, SelectPlan};
use crate::sql::{self, Select, Statement};
use crate::{Error, Outcome, Resource, Result, SelectExplanation, Span};

impl Database {
    /// Parse and execute exactly one SQL statement.
    pub fn execute(&mut self, sql: &str) -> Result<Outcome> {
        self.check_request(sql)?;
        let statement = sql::parse(sql)?;
        match statement {
            Statement::CreateTable(statement) => self.execute_create(statement),
            Statement::Insert(statement) => self.execute_insert(statement),
            Statement::Select(statement) => {
                let plan = self.compile_select_ast(&statement)?;
                query::execute_select(self.storage.as_str(), &plan, &self.limits).map(Outcome::Rows)
            }
            Statement::Update(statement) => self.execute_update(statement),
            Statement::Delete(statement) => self.execute_delete(statement),
            Statement::ExplainRegex(statement) => self
                .compile_select_ast(&statement)?
                .into_explanation(self.limits.max_query_output_bytes)
                .map(Outcome::Explain),
        }
    }

    /// Parse, resolve, and explain a `SELECT`'s source-row scans.
    pub fn explain_select(&self, sql: &str) -> Result<SelectExplanation> {
        self.check_request(sql)?;
        match sql::parse(sql)? {
            Statement::Select(statement) => self
                .compile_select_ast(&statement)
                .and_then(|plan| plan.into_explanation(self.limits.max_query_output_bytes)),
            _ => Err(Error::unsupported(
                "explain_select expects a SELECT statement",
                Span::new(0, sql.len()),
            )),
        }
    }

    fn check_request(&self, sql: &str) -> Result<()> {
        check_limit(
            self.storage.as_str().len(),
            self.limits.max_database_bytes,
            Resource::DatabaseBytes,
        )?;
        check_limit(sql.len(), self.limits.max_sql_bytes, Resource::SqlBytes)
    }

    fn compile_select_ast<'statement>(
        &self,
        statement: &'statement Select,
    ) -> Result<SelectPlan<'_, 'statement>> {
        query::compile_select(self.storage.catalog(), statement, &self.limits)
    }
}
