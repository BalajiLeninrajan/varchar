//! Public database façade and atomic statement orchestration.

use std::fmt;

use crate::limits::{Limits, check_limit};
use crate::query::{self, SelectPlan};
use crate::resolve;
use crate::sql::{self, CreateTable, Delete, Insert, Select, Statement, Update};
use crate::storage;
use crate::{Error, Outcome, RegexPlan, Result, Span};

/// An in-memory database whose sole authoritative state is one UTF-8 string.
#[derive(Clone)]
pub struct Database {
    blob: String,
    catalog: storage::Catalog,
    limits: Limits,
}

impl fmt::Debug for Database {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Database")
            .field("blob", &self.blob)
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
            blob: storage::EMPTY_BLOB.to_owned(),
            catalog: storage::Catalog::empty(),
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
        let catalog = storage::validate_and_catalog(&blob)?;
        Ok(Self {
            blob,
            catalog,
            limits,
        })
    }

    /// Borrow the canonical authoritative database string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.blob
    }

    /// Consume the database and return its authoritative string.
    #[must_use]
    pub fn into_string(self) -> String {
        self.blob
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
                query::execute_select(&self.blob, &plan, &self.limits).map(Outcome::Rows)
            }
            Statement::Update(statement) => self.execute_update(statement),
            Statement::Delete(statement) => self.execute_delete(statement),
            Statement::ExplainRegex(statement) => self
                .compile_select_ast(&statement)
                .map(|plan| Outcome::Explain(plan.into_regex_plan())),
        }
    }

    /// Parse, resolve, and compile a `SELECT` into its exact row-selection regex.
    pub fn compile_select(&self, sql: &str) -> Result<RegexPlan> {
        self.check_request(sql)?;
        match sql::parse(sql)? {
            Statement::Select(statement) => self
                .compile_select_ast(&statement)
                .map(SelectPlan::into_regex_plan),
            _ => Err(Error::parse(
                "compile_select expects a SELECT statement",
                Span::new(0, sql.len()),
            )),
        }
    }

    fn check_request(&self, sql: &str) -> Result<()> {
        check_limit(
            self.blob.len(),
            self.limits.max_database_bytes,
            "database bytes",
        )?;
        check_limit(sql.len(), self.limits.max_sql_bytes, "SQL bytes")
    }

    fn execute_create(&mut self, statement: CreateTable) -> Result<Outcome> {
        let schema = resolve::create_schema(&self.catalog, statement)?;
        let table = schema.name.clone();
        let mut candidate = storage::Candidate::new(&self.blob, self.limits.max_database_bytes)?;
        candidate.insert_schema(&self.catalog, &schema)?;
        self.commit_candidate(candidate.finish()?)?;
        Ok(Outcome::Created { table })
    }

    fn execute_insert(&mut self, statement: Insert) -> Result<Outcome> {
        let schema = resolve::require_table(&self.catalog, &statement.table)?;
        let values = resolve::insert_values(schema, statement.columns, statement.values)?;
        let mut candidate = storage::Candidate::new(&self.blob, self.limits.max_database_bytes)?;
        candidate.append_row(schema.row_layout(), &values)?;
        self.commit_candidate(candidate.finish()?)?;
        Ok(Outcome::Affected { rows: 1 })
    }

    fn execute_update(&mut self, statement: Update) -> Result<Outcome> {
        let schema = resolve::require_table(&self.catalog, &statement.table)?;
        let assignments = resolve::assignments(schema, &statement.assignments)?;
        let plan = query::compile_scan(schema, &statement.predicates, &self.limits)?;
        let (candidate, affected) =
            query::rewrite_matching_rows(&self.blob, &plan, &self.limits, |mut values| {
                for (index, value) in &assignments {
                    values[*index] = value.clone();
                }
                Ok(Some(values))
            })?;
        self.commit_candidate(candidate)?;
        Ok(Outcome::Affected { rows: affected })
    }

    fn execute_delete(&mut self, statement: Delete) -> Result<Outcome> {
        let schema = resolve::require_table(&self.catalog, &statement.table)?;
        let plan = query::compile_scan(schema, &statement.predicates, &self.limits)?;
        let (candidate, affected) =
            query::rewrite_matching_rows(&self.blob, &plan, &self.limits, |_| Ok(None))?;
        self.commit_candidate(candidate)?;
        Ok(Outcome::Affected { rows: affected })
    }

    fn compile_select_ast(&self, statement: &Select) -> Result<SelectPlan> {
        query::compile_select(&self.catalog, statement, &self.limits)
    }

    fn commit_candidate(&mut self, candidate: String) -> Result<()> {
        check_limit(
            candidate.len(),
            self.limits.max_database_bytes,
            "database bytes",
        )?;
        let next_catalog = storage::validate_and_catalog(&candidate)?;
        (self.blob, self.catalog) = (candidate, next_catalog);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Database;
    use crate::{Error, storage};

    fn assert_catalog_current(database: &Database) {
        let reconstructed =
            storage::validate_and_catalog(database.as_str()).expect("database remains valid");
        assert_eq!(database.catalog, reconstructed);
    }

    #[test]
    fn derived_catalog_tracks_every_commit() {
        let mut database = Database::new();
        assert_catalog_current(&database);

        for sql in [
            "CREATE TABLE t (id INTEGER NOT NULL, note TEXT)",
            "INSERT INTO t VALUES (1, 'first')",
            "CREATE TABLE flags (enabled BOOLEAN NOT NULL)",
            "UPDATE t SET note = 'changed' WHERE id = 1",
            "DELETE FROM t WHERE id = 1",
        ] {
            database.execute(sql).expect("statement succeeds");
            assert_catalog_current(&database);
        }
    }

    #[test]
    fn failed_candidate_validation_preserves_blob_and_catalog() {
        let mut database = Database::new();
        database
            .execute("CREATE TABLE t (id INTEGER)")
            .expect("fixture schema succeeds");
        let before_blob = database.blob.clone();
        let before_catalog = database.catalog.clone();

        assert!(matches!(
            database.commit_candidate(String::from("V1;garbage")),
            Err(Error::CorruptStorage { .. })
        ));
        assert_eq!(database.blob, before_blob);
        assert_eq!(database.catalog, before_catalog);
    }

    #[test]
    fn debug_output_omits_the_derived_catalog() {
        let database = Database::new();
        assert!(!format!("{database:?}").contains("catalog"));
    }
}
