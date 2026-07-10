//! SQL execution over the authoritative one-string database.

use std::collections::BTreeSet;

use fancy_regex::{Error as FancyError, Regex, RegexBuilder, RuntimeError};

use crate::sql::{
    self, Assignment, CreateTable, Delete, Insert, Predicate, PredicateOperator, Projection,
    Select, Statement, Update,
};
use crate::storage::{self, TableSchema};
use crate::{Column, DataType, Error, Outcome, RegexPlan, Result, RowSet, Span, Value};

const MIB: usize = 1024 * 1024;
const TEXT_UNIT_PATTERN: &str = r"(?:%[0-9A-F]{6}|[^%|;~])";

/// Resource bounds applied by the platform-neutral database core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Maximum size of the authoritative database string, in UTF-8 bytes.
    pub max_database_bytes: usize,
    /// Maximum size of one SQL statement, in UTF-8 bytes.
    pub max_sql_bytes: usize,
    /// Maximum number of `WHERE` terms joined by `AND`.
    pub max_predicates: usize,
    /// Maximum size of a generated regular expression, in UTF-8 bytes.
    pub max_pattern_bytes: usize,
    /// Maximum amount of typed value data materialized by a query.
    pub max_result_bytes: usize,
    /// Per-search backtracking limit passed to the regex engine.
    pub regex_backtrack_limit: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_database_bytes: 64 * MIB,
            max_sql_bytes: 64 * 1024,
            max_predicates: 64,
            max_pattern_bytes: 8 * MIB,
            max_result_bytes: 64 * MIB,
            regex_backtrack_limit: 1_000_000,
        }
    }
}

/// An in-memory database whose sole authoritative state is one UTF-8 string.
#[derive(Clone, Debug)]
pub struct Database {
    blob: String,
    limits: Limits,
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
        storage::validate_and_catalog(&blob)?;
        Ok(Self { blob, limits })
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
                execute_plan(&self.blob, &plan, &self.limits).map(Outcome::Rows)
            }
            Statement::Update(statement) => self.execute_update(statement),
            Statement::Delete(statement) => self.execute_delete(statement),
            Statement::ExplainRegex(statement) => {
                self.compile_select_ast(&statement).map(Outcome::Explain)
            }
        }
    }

    /// Parse, resolve, and compile a `SELECT` into its exact row-selection regex.
    pub fn compile_select(&self, sql: &str) -> Result<RegexPlan> {
        self.check_request(sql)?;
        match sql::parse(sql)? {
            Statement::Select(statement) => self.compile_select_ast(&statement),
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
        let catalog = storage::validate_and_catalog(&self.blob)?;
        if catalog.table(&statement.table).is_some() {
            return Err(Error::Schema(format!(
                "table {:?} already exists",
                statement.table
            )));
        }

        let schema = TableSchema {
            name: statement.table.clone(),
            columns: statement
                .columns
                .into_iter()
                .map(|column| Column {
                    name: column.name,
                    data_type: column.data_type,
                    nullable: column.nullable,
                })
                .collect(),
        };
        let encoded = storage::encode_schema(&schema)?;
        let mut candidate = String::new();
        push_database_fragment(
            &mut candidate,
            &self.blob[..catalog.row_start],
            &self.limits,
        )?;
        push_database_fragment(&mut candidate, &encoded, &self.limits)?;
        push_database_fragment(
            &mut candidate,
            &self.blob[catalog.row_start..],
            &self.limits,
        )?;
        self.commit_candidate(candidate)?;
        Ok(Outcome::Created {
            table: statement.table,
        })
    }

    fn execute_insert(&mut self, statement: Insert) -> Result<Outcome> {
        let catalog = storage::validate_and_catalog(&self.blob)?;
        let schema = require_table(&catalog, &statement.table)?;
        let values = arrange_insert_values(schema, statement.columns, statement.values)?;
        let encoded = storage::encode_row(&statement.table, &values, schema)?;

        let mut candidate = String::new();
        push_database_fragment(&mut candidate, &self.blob, &self.limits)?;
        push_database_fragment(&mut candidate, &encoded, &self.limits)?;
        self.commit_candidate(candidate)?;
        Ok(Outcome::Affected { rows: 1 })
    }

    fn execute_update(&mut self, statement: Update) -> Result<Outcome> {
        let catalog = storage::validate_and_catalog(&self.blob)?;
        let schema = require_table(&catalog, &statement.table)?;
        let assignments = compile_assignments(schema, &statement.assignments)?;
        let predicates = compile_predicates(schema, &statement.predicates, &self.limits)?;
        let projection = (0..schema.columns.len()).collect();
        let plan = make_plan(
            &statement.table,
            schema,
            projection,
            predicates,
            &self.limits,
        )?;
        let (candidate, affected) = rewrite_matching_rows(
            &self.blob,
            schema,
            plan.pattern(),
            &self.limits,
            |mut values| {
                for (index, value) in &assignments {
                    values[*index] = value.clone();
                }
                Ok(Some(values))
            },
        )?;
        self.commit_candidate(candidate)?;
        Ok(Outcome::Affected { rows: affected })
    }

    fn execute_delete(&mut self, statement: Delete) -> Result<Outcome> {
        let catalog = storage::validate_and_catalog(&self.blob)?;
        let schema = require_table(&catalog, &statement.table)?;
        let predicates = compile_predicates(schema, &statement.predicates, &self.limits)?;
        let projection = (0..schema.columns.len()).collect();
        let plan = make_plan(
            &statement.table,
            schema,
            projection,
            predicates,
            &self.limits,
        )?;
        let (candidate, affected) =
            rewrite_matching_rows(&self.blob, schema, plan.pattern(), &self.limits, |_| {
                Ok(None)
            })?;
        self.commit_candidate(candidate)?;
        Ok(Outcome::Affected { rows: affected })
    }

    fn compile_select_ast(&self, statement: &Select) -> Result<RegexPlan> {
        let catalog = storage::validate_and_catalog(&self.blob)?;
        let schema = require_table(&catalog, &statement.table)?;
        let projection = resolve_projection(schema, &statement.projection)?;
        let predicates = compile_predicates(schema, &statement.predicates, &self.limits)?;
        make_plan(
            &statement.table,
            schema,
            projection,
            predicates,
            &self.limits,
        )
    }

    fn commit_candidate(&mut self, candidate: String) -> Result<()> {
        check_limit(
            candidate.len(),
            self.limits.max_database_bytes,
            "database bytes",
        )?;
        storage::validate_and_catalog(&candidate)?;
        self.blob = candidate;
        Ok(())
    }
}

#[derive(Clone, Debug)]
enum CompiledPredicate {
    Equal { column: usize, encoded: String },
    NotEqual { column: usize, encoded: String },
    Like { column: usize, pattern: String },
    IsNull { column: usize },
    IsNotNull { column: usize },
}

fn require_table<'a>(catalog: &'a storage::Catalog, table: &str) -> Result<&'a TableSchema> {
    catalog
        .table(table)
        .ok_or_else(|| Error::Schema(format!("unknown table {table:?}")))
}

fn resolve_projection(schema: &TableSchema, projection: &Projection) -> Result<Vec<usize>> {
    match projection {
        Projection::All => Ok((0..schema.columns.len()).collect()),
        Projection::Columns(columns) => columns
            .iter()
            .map(|name| require_column(schema, name))
            .collect(),
    }
}

fn require_column(schema: &TableSchema, name: &str) -> Result<usize> {
    schema
        .columns
        .iter()
        .position(|column| column.name == name)
        .ok_or_else(|| {
            Error::Schema(format!(
                "unknown column {name:?} in table {:?}",
                schema.name
            ))
        })
}

fn arrange_insert_values(
    schema: &TableSchema,
    columns: Option<Vec<String>>,
    supplied: Vec<Value>,
) -> Result<Vec<Value>> {
    let values = if let Some(columns) = columns {
        if columns.len() != supplied.len() {
            return Err(Error::Type(format!(
                "INSERT names {} columns but supplies {} values",
                columns.len(),
                supplied.len()
            )));
        }
        let mut seen = BTreeSet::new();
        let mut values = vec![Value::Null; schema.columns.len()];
        for (name, value) in columns.into_iter().zip(supplied) {
            if !seen.insert(name.clone()) {
                return Err(Error::Schema(format!("duplicate INSERT column {name:?}")));
            }
            let index = require_column(schema, &name)?;
            values[index] = value;
        }
        values
    } else {
        if supplied.len() != schema.columns.len() {
            return Err(Error::Type(format!(
                "table {:?} expects {} values, got {}",
                schema.name,
                schema.columns.len(),
                supplied.len()
            )));
        }
        supplied
    };

    // Encoding performs both type and nullability validation.
    for (value, column) in values.iter().zip(&schema.columns) {
        let _ = storage::encode_cell(value, column)?;
    }
    Ok(values)
}

fn compile_assignments(
    schema: &TableSchema,
    assignments: &[Assignment],
) -> Result<Vec<(usize, Value)>> {
    let mut compiled = Vec::with_capacity(assignments.len());
    let mut seen = BTreeSet::new();
    for assignment in assignments {
        if !seen.insert(assignment.column.as_str()) {
            return Err(Error::Schema(format!(
                "duplicate UPDATE assignment for column {:?}",
                assignment.column
            )));
        }
        let index = require_column(schema, &assignment.column)?;
        let _ = storage::encode_cell(&assignment.value, &schema.columns[index])?;
        compiled.push((index, assignment.value.clone()));
    }
    Ok(compiled)
}

fn compile_predicates(
    schema: &TableSchema,
    predicates: &[Predicate],
    limits: &Limits,
) -> Result<Vec<CompiledPredicate>> {
    check_limit(predicates.len(), limits.max_predicates, "WHERE predicates")?;
    predicates
        .iter()
        .map(|predicate| {
            let column = require_column(schema, &predicate.column)?;
            let definition = &schema.columns[column];
            match &predicate.operator {
                PredicateOperator::Equal(Value::Null)
                | PredicateOperator::NotEqual(Value::Null) => Err(Error::Type(String::from(
                    "NULL cannot be compared with `=` or `!=`; use IS NULL or IS NOT NULL",
                ))),
                PredicateOperator::Equal(value) => {
                    let encoded = storage::encode_cell(value, definition)?;
                    Ok(CompiledPredicate::Equal { column, encoded })
                }
                PredicateOperator::NotEqual(value) => {
                    let encoded = storage::encode_cell(value, definition)?;
                    Ok(CompiledPredicate::NotEqual { column, encoded })
                }
                PredicateOperator::Like(pattern) => {
                    if definition.data_type != DataType::Text {
                        return Err(Error::Type(format!(
                            "LIKE requires a TEXT column; {:?} is {}",
                            definition.name, definition.data_type
                        )));
                    }
                    Ok(CompiledPredicate::Like {
                        column,
                        pattern: compile_like_pattern(pattern, definition, limits)?,
                    })
                }
                PredicateOperator::IsNull => Ok(CompiledPredicate::IsNull { column }),
                PredicateOperator::IsNotNull => Ok(CompiledPredicate::IsNotNull { column }),
            }
        })
        .collect()
}

fn compile_row_pattern(
    table: &str,
    schema: &[Column],
    predicates: &[CompiledPredicate],
    limits: &Limits,
) -> Result<String> {
    check_limit(predicates.len(), limits.max_predicates, "WHERE predicates")?;

    let mut pattern = PatternBuilder::new(limits.max_pattern_bytes);
    pattern.push_str(r"~R\|")?;
    pattern.push_str(&regex::escape(table))?;
    pattern.push_str(r"\|")?;
    for predicate in predicates {
        let column_index = predicate.column();
        pattern.push_str("(?=")?;
        for column in &schema[..column_index] {
            pattern.push_str(&cell_pattern(column, true))?;
            pattern.push_str(r"\|")?;
        }
        match predicate {
            CompiledPredicate::Equal { encoded, .. } => {
                pattern.push_str(&regex::escape(encoded))?;
            }
            CompiledPredicate::NotEqual { encoded, .. } => {
                pattern.push_str("(?!")?;
                pattern.push_str(&regex::escape(encoded))?;
                pattern.push_str(cell_boundary(column_index, schema.len()))?;
                pattern.push_char(')')?;
                pattern.push_str(&cell_pattern(&schema[column_index], false))?;
            }
            CompiledPredicate::Like {
                pattern: like_pattern,
                ..
            } => pattern.push_str(like_pattern)?,
            CompiledPredicate::IsNull { .. } => pattern.push_char('N')?,
            CompiledPredicate::IsNotNull { .. } => {
                pattern.push_str(&cell_pattern(&schema[column_index], false))?;
            }
        }
        pattern.push_str(cell_boundary(column_index, schema.len()))?;
        pattern.push_char(')')?;
    }

    for (index, column) in schema.iter().enumerate() {
        if index > 0 {
            pattern.push_str(r"\|")?;
        }
        pattern.push_str(&cell_pattern(column, true))?;
    }
    pattern.push_char(';')?;
    Ok(pattern.finish())
}

struct PatternBuilder {
    pattern: String,
    limit: usize,
}

impl PatternBuilder {
    fn new(limit: usize) -> Self {
        Self {
            pattern: String::new(),
            limit,
        }
    }

    fn push_str(&mut self, fragment: &str) -> Result<()> {
        let new_len =
            self.pattern
                .len()
                .checked_add(fragment.len())
                .ok_or(Error::ResourceLimit {
                    resource: "generated regex bytes",
                    limit: self.limit,
                })?;
        check_limit(new_len, self.limit, "generated regex bytes")?;
        self.pattern
            .try_reserve(fragment.len())
            .map_err(|_| Error::ResourceLimit {
                resource: "generated regex bytes",
                limit: self.limit,
            })?;
        self.pattern.push_str(fragment);
        Ok(())
    }

    fn push_char(&mut self, character: char) -> Result<()> {
        let mut encoded = [0_u8; 4];
        self.push_str(character.encode_utf8(&mut encoded))
    }

    fn finish(self) -> String {
        self.pattern
    }
}

impl CompiledPredicate {
    const fn column(&self) -> usize {
        match self {
            Self::Equal { column, .. }
            | Self::NotEqual { column, .. }
            | Self::Like { column, .. }
            | Self::IsNull { column }
            | Self::IsNotNull { column } => *column,
        }
    }
}

fn cell_boundary(column: usize, column_count: usize) -> &'static str {
    if column + 1 == column_count {
        ";"
    } else {
        r"\|"
    }
}

fn cell_pattern(column: &Column, include_null: bool) -> String {
    let typed = match column.data_type {
        DataType::Text => format!("T{TEXT_UNIT_PATTERN}*"),
        DataType::Integer => String::from(r"I(?:0|-?[1-9][0-9]*)"),
        DataType::Boolean => String::from(r"B[01]"),
    };
    if include_null && column.nullable {
        format!("(?:N|{typed})")
    } else {
        typed
    }
}

fn compile_like_pattern(value: &str, column: &Column, limits: &Limits) -> Result<String> {
    let mut result = PatternBuilder::new(limits.max_pattern_bytes);
    result.push_str("T")?;
    let mut characters = value.chars().peekable();
    let mut previous_was_many = false;
    while let Some(character) = characters.next() {
        match character {
            '%' => {
                if !previous_was_many {
                    result.push_str(TEXT_UNIT_PATTERN)?;
                    result.push_char('*')?;
                    previous_was_many = true;
                }
            }
            '_' => {
                result.push_str(TEXT_UNIT_PATTERN)?;
                previous_was_many = false;
            }
            '\\' => {
                let Some(escaped) = characters.next() else {
                    return Err(Error::Type(String::from(
                        "LIKE pattern ends with an incomplete escape",
                    )));
                };
                if !matches!(escaped, '%' | '_' | '\\') {
                    return Err(Error::Type(format!(
                        "LIKE pattern contains unsupported escape \\{escaped}"
                    )));
                }
                push_encoded_text_literal(&mut result, escaped, column)?;
                previous_was_many = false;
            }
            literal => {
                push_encoded_text_literal(&mut result, literal, column)?;
                previous_was_many = false;
            }
        }
    }
    Ok(result.finish())
}

fn push_encoded_text_literal(
    result: &mut PatternBuilder,
    character: char,
    column: &Column,
) -> Result<()> {
    let encoded = storage::encode_cell(&Value::Text(character.to_string()), column)?;
    let payload = encoded
        .strip_prefix('T')
        .expect("encoding a TEXT value always produces a T-prefixed cell");
    result.push_str(&regex::escape(payload))
}

fn build_regex(pattern: &str, limits: &Limits) -> Result<Regex> {
    let mut builder = RegexBuilder::new(pattern);
    builder
        .backtrack_limit(limits.regex_backtrack_limit)
        .delegate_size_limit(limits.max_pattern_bytes);
    builder
        .build()
        .map_err(|error| Error::RegexCompile(error.to_string()))
}

fn execute_plan(blob: &str, plan: &RegexPlan, limits: &Limits) -> Result<RowSet> {
    let regex = build_regex(plan.pattern(), limits)?;
    let schema = TableSchema {
        name: plan.table.clone(),
        columns: plan.schema.clone(),
    };
    let mut result_bytes = std::mem::size_of::<RowSet>();
    check_limit(result_bytes, limits.max_result_bytes, "result bytes")?;

    let column_slots = plan
        .projection
        .len()
        .checked_mul(std::mem::size_of::<Column>())
        .ok_or_else(|| result_limit_error(limits))?;
    charge_result(&mut result_bytes, column_slots, limits)?;

    let mut columns = Vec::new();
    columns
        .try_reserve_exact(plan.projection.len())
        .map_err(|_| result_limit_error(limits))?;
    for &index in &plan.projection {
        let column = &plan.schema[index];
        charge_result(&mut result_bytes, column.name.len(), limits)?;
        let mut name = String::new();
        name.try_reserve_exact(column.name.len())
            .map_err(|_| result_limit_error(limits))?;
        name.push_str(&column.name);
        columns.push(Column {
            name,
            data_type: column.data_type,
            nullable: column.nullable,
        });
    }

    let mut rows = Vec::new();
    let value_slots = plan
        .projection
        .len()
        .checked_mul(std::mem::size_of::<Value>())
        .ok_or_else(|| result_limit_error(limits))?;
    // Vec growth may reserve more outer row slots than are immediately used. Charging
    // four row descriptors per returned row keeps the byte budget conservative.
    let row_descriptors = std::mem::size_of::<Vec<Value>>()
        .checked_mul(4)
        .ok_or_else(|| result_limit_error(limits))?;
    let row_structure = row_descriptors
        .checked_add(value_slots)
        .ok_or_else(|| result_limit_error(limits))?;

    for matched in regex.find_iter(blob) {
        let matched = matched.map_err(|error| map_regex_runtime(error, limits))?;
        let structural_total = result_bytes
            .checked_add(row_structure)
            .ok_or_else(|| result_limit_error(limits))?;
        check_limit(structural_total, limits.max_result_bytes, "result bytes")?;

        let decoded = storage::decode_row(matched.as_str(), &schema)?;
        let payload_bytes = plan.projection.iter().try_fold(0_usize, |total, &index| {
            total
                .checked_add(value_payload_size(&decoded[index]))
                .ok_or_else(|| result_limit_error(limits))
        })?;
        let row_charge = row_structure
            .checked_add(payload_bytes)
            .ok_or_else(|| result_limit_error(limits))?;
        charge_result(&mut result_bytes, row_charge, limits)?;

        rows.try_reserve(1)
            .map_err(|_| result_limit_error(limits))?;
        let mut row = Vec::new();
        row.try_reserve_exact(plan.projection.len())
            .map_err(|_| result_limit_error(limits))?;
        for &index in &plan.projection {
            row.push(clone_result_value(&decoded[index], limits)?);
        }
        rows.push(row);
    }

    Ok(RowSet { columns, rows })
}

fn rewrite_matching_rows<F>(
    blob: &str,
    schema: &TableSchema,
    pattern: &str,
    limits: &Limits,
    mut rewrite: F,
) -> Result<(String, usize)>
where
    F: FnMut(Vec<Value>) -> Result<Option<Vec<Value>>>,
{
    let regex = build_regex(pattern, limits)?;
    let mut candidate = String::new();
    candidate
        .try_reserve(blob.len())
        .map_err(|_| Error::ResourceLimit {
            resource: "database bytes",
            limit: limits.max_database_bytes,
        })?;
    let mut previous_end = 0;
    let mut affected = 0_usize;

    for matched in regex.find_iter(blob) {
        let matched = matched.map_err(|error| map_regex_runtime(error, limits))?;
        push_database_fragment(&mut candidate, &blob[previous_end..matched.start()], limits)?;
        let values = storage::decode_row(matched.as_str(), schema)?;
        if let Some(values) = rewrite(values)? {
            let encoded = storage::encode_row(&schema.name, &values, schema)?;
            push_database_fragment(&mut candidate, &encoded, limits)?;
        }
        previous_end = matched.end();
        affected = affected.checked_add(1).ok_or(Error::ResourceLimit {
            resource: "affected rows",
            limit: usize::MAX,
        })?;
    }
    push_database_fragment(&mut candidate, &blob[previous_end..], limits)?;
    Ok((candidate, affected))
}

fn push_database_fragment(candidate: &mut String, fragment: &str, limits: &Limits) -> Result<()> {
    let new_len = candidate
        .len()
        .checked_add(fragment.len())
        .ok_or(Error::ResourceLimit {
            resource: "database bytes",
            limit: limits.max_database_bytes,
        })?;
    check_limit(new_len, limits.max_database_bytes, "database bytes")?;
    candidate.push_str(fragment);
    Ok(())
}

fn make_plan(
    table: &str,
    schema: &TableSchema,
    projection: Vec<usize>,
    predicates: Vec<CompiledPredicate>,
    limits: &Limits,
) -> Result<RegexPlan> {
    let pattern = compile_row_pattern(table, &schema.columns, &predicates, limits)?;
    // Compile eagerly so `compile_select` never returns an unusable plan.
    let _ = build_regex(&pattern, limits)?;
    Ok(RegexPlan {
        pattern,
        table: table.to_owned(),
        schema: schema.columns.clone(),
        projection,
    })
}

fn value_payload_size(value: &Value) -> usize {
    match value {
        Value::Text(value) => value.len(),
        Value::Integer(_) | Value::Boolean(_) | Value::Null => 0,
    }
}

fn clone_result_value(value: &Value, limits: &Limits) -> Result<Value> {
    match value {
        Value::Text(value) => {
            let mut cloned = String::new();
            cloned
                .try_reserve_exact(value.len())
                .map_err(|_| result_limit_error(limits))?;
            cloned.push_str(value);
            Ok(Value::Text(cloned))
        }
        Value::Integer(value) => Ok(Value::Integer(*value)),
        Value::Boolean(value) => Ok(Value::Boolean(*value)),
        Value::Null => Ok(Value::Null),
    }
}

fn charge_result(total: &mut usize, amount: usize, limits: &Limits) -> Result<()> {
    *total = total
        .checked_add(amount)
        .ok_or_else(|| result_limit_error(limits))?;
    check_limit(*total, limits.max_result_bytes, "result bytes")
}

fn result_limit_error(limits: &Limits) -> Error {
    Error::ResourceLimit {
        resource: "result bytes",
        limit: limits.max_result_bytes,
    }
}

fn map_regex_runtime(error: FancyError, limits: &Limits) -> Error {
    match error {
        FancyError::RuntimeError(
            RuntimeError::BacktrackLimitExceeded | RuntimeError::StackOverflow,
        ) => Error::ResourceLimit {
            resource: "regex execution steps",
            limit: limits.regex_backtrack_limit,
        },
        other => Error::RegexRuntime(other.to_string()),
    }
}

fn check_limit(actual: usize, limit: usize, resource: &'static str) -> Result<()> {
    if actual > limit {
        Err(Error::ResourceLimit { resource, limit })
    } else {
        Ok(())
    }
}
