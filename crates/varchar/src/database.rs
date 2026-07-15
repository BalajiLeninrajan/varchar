//! Public database façade and atomic statement orchestration.

use std::fmt;

use crate::limits::{Limits, check_limit};
use crate::query::{self, SelectPlan};
use crate::resolve;
use crate::sql::{self, CreateTable, Delete, Insert, Select, Statement, Update};
use crate::storage;
use crate::{Error, Outcome, Result, SelectExplanation, Span};

/// An in-memory database whose sole authoritative state is one UTF-8 string.
#[derive(Clone)]
pub struct Database {
    storage: storage::StorageState,
    limits: Limits,
}

impl fmt::Debug for Database {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Database")
            .field("blob", &self.storage.as_str())
            .field("limits", &self.limits)
            .finish()
    }
}

impl Default for Database {
    fn default() -> Self {
        Self::new()
    }
}

impl Database {
    /// Construct an empty database with the default resource limits.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    /// Construct an empty database with caller-supplied resource limits.
    #[must_use]
    pub fn with_limits(limits: Limits) -> Self {
        Self {
            storage: storage::StorageState::empty(),
            limits,
        }
    }

    /// Validate and load an authoritative database string.
    pub fn from_string(blob: String) -> Result<Self> {
        Self::from_string_with_limits(blob, Limits::default())
    }

    /// Validate and load a database string with caller-supplied limits.
    pub fn from_string_with_limits(blob: String, limits: Limits) -> Result<Self> {
        check_limit(blob.len(), limits.max_database_bytes, "database bytes")?;
        let storage = storage::StorageState::load(blob)?;
        Ok(Self { storage, limits })
    }

    /// Borrow the canonical authoritative database string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.storage.as_str()
    }

    /// Consume the database and return its authoritative string.
    #[must_use]
    pub fn into_string(self) -> String {
        self.storage.into_string()
    }

    /// Resource limits used by this database.
    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

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
            _ => Err(Error::parse(
                "explain_select expects a SELECT statement",
                Span::new(0, sql.len()),
            )),
        }
    }

    fn check_request(&self, sql: &str) -> Result<()> {
        check_limit(
            self.storage.as_str().len(),
            self.limits.max_database_bytes,
            "database bytes",
        )?;
        check_limit(sql.len(), self.limits.max_sql_bytes, "SQL bytes")
    }

    fn execute_create(&mut self, statement: CreateTable) -> Result<Outcome> {
        let resolved = resolve::create_schema(self.storage.catalog(), statement)?;
        let table = resolved.schema.name.clone();
        let mut candidate = self.storage.candidate(self.limits.max_database_bytes)?;
        candidate.insert_schema_with_auto_increment(&resolved.schema, resolved.auto_increment)?;
        self.storage = candidate.finish()?;
        Ok(Outcome::Created { table })
    }

    fn execute_insert(&mut self, statement: Insert) -> Result<Outcome> {
        let schema = resolve::require_table(self.storage.catalog(), &statement.table)?;
        let auto_increment = self.storage.catalog().auto_increment(&statement.table);
        let resolved =
            resolve::insert_values(schema, auto_increment, statement.columns, statement.values)?;
        let mut candidate = self.storage.candidate(self.limits.max_database_bytes)?;
        if let Some(last) = resolved.next_auto_increment {
            candidate.advance_auto_increment(&statement.table, last)?;
        }
        candidate.append_row(schema.row_layout(), &resolved.values)?;
        self.storage = candidate.finish()?;
        Ok(Outcome::Affected { rows: 1 })
    }

    fn execute_update(&mut self, statement: Update) -> Result<Outcome> {
        let schema = resolve::require_table(self.storage.catalog(), &statement.table)?;
        let auto_increment = self.storage.catalog().auto_increment(&statement.table);
        let assignments = resolve::assignments(schema, auto_increment, &statement.assignments)?;
        let plan = query::compile_scan(schema, &statement.predicates, &self.limits)?;
        let mut candidate = self.storage.candidate(self.limits.max_database_bytes)?;
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

    fn execute_delete(&mut self, statement: Delete) -> Result<Outcome> {
        let schema = resolve::require_table(self.storage.catalog(), &statement.table)?;
        let plan = query::compile_scan(schema, &statement.predicates, &self.limits)?;
        let mut candidate = self.storage.candidate(self.limits.max_database_bytes)?;
        let affected =
            query::rewrite_matching_rows(&mut candidate, &plan, &self.limits, |_| Ok(None))?;
        self.storage = candidate.finish()?;
        Ok(Outcome::Affected { rows: affected })
    }

    fn compile_select_ast(&self, statement: &Select) -> Result<SelectPlan<'_>> {
        query::compile_select(self.storage.catalog(), statement, &self.limits)
    }
}

#[cfg(test)]
mod tests;
