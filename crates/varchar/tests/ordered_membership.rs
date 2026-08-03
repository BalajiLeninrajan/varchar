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

#[path = "ordered_membership/parser_diagnostics.rs"]
mod parser_diagnostics;
#[path = "ordered_membership/semantics.rs"]
mod semantics;
#[path = "ordered_membership/type_errors.rs"]
mod type_errors;
