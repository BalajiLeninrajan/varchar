//! Execution of compiled scans through selection or mutation rewriting.

mod rewrite;
mod select;

use fancy_regex::{Error as FancyError, RuntimeError};

use crate::limits::Limits;
use crate::{Error, Resource};

pub(crate) use rewrite::rewrite_matching_rows;
pub(super) use select::explain;
pub(crate) use select::select as execute_select;

fn map_regex_runtime(error: FancyError, limits: &Limits) -> Error {
    match error {
        FancyError::RuntimeError(RuntimeError::BacktrackLimitExceeded) => Error::ResourceLimit {
            resource: Resource::RegexBacktracking,
            limit: limits.regex_backtrack_limit,
        },
        other => Error::RegexRuntime(other.to_string()),
    }
}

#[cfg(test)]
mod tests;
