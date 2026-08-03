#![cfg(not(target_family = "wasm"))]

use varchar::{Database, Error};

#[path = "boolean_expressions/semantics.rs"]
mod semantics;
