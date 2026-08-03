#![cfg(not(target_family = "wasm"))]

use varchar::{Database, Error, Limits, Outcome, Resource, RowSet, Value};

fn execute(database: &mut Database, sql: &str) -> Outcome {
    database
        .execute(sql)
        .unwrap_or_else(|error| panic!("failed to execute {sql:?}: {error}"))
}

fn rows(database: &mut Database, sql: &str) -> RowSet {
    match execute(database, sql) {
        Outcome::Rows(rows) => rows,
        other => panic!("expected rows for {sql:?}, got {other:?}"),
    }
}

#[path = "ordered_membership/execution.rs"]
mod execution;
#[path = "ordered_membership/identifiers.rs"]
mod identifiers;
#[path = "ordered_membership/limits.rs"]
mod limits;
#[path = "ordered_membership/parser_diagnostics.rs"]
mod parser_diagnostics;
#[path = "ordered_membership/semantics.rs"]
mod semantics;
#[path = "ordered_membership/type_errors.rs"]
mod type_errors;
