//! Query planning, physical compilation, and row execution.

mod compile;
mod execute;
mod pattern;
mod plan;

pub(crate) use compile::{compile_scan, compile_select};
pub(crate) use execute::{execute_select, rewrite_matching_rows};
pub(crate) use plan::{ScanPlan, SelectPlan};

#[cfg(test)]
mod tests;
