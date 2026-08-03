#![cfg(not(target_family = "wasm"))]

use varchar::{Database, Error, Outcome, RowSet, Value};

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

#[path = "boolean_expressions/execution.rs"]
mod execution;
#[path = "boolean_expressions/semantics.rs"]
mod semantics;
