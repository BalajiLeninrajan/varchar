//! Query planning, physical compilation, and row execution.

mod compile;
mod execute;
mod pattern;
mod plan;
mod pushdown;

pub(crate) use compile::{compile_scan, compile_select};
pub(crate) use execute::{execute_select, map_regex_runtime, rewrite_matching_rows};
pub(crate) use plan::{ScanPlan, SelectPlan};

#[cfg(test)]
mod tests;
