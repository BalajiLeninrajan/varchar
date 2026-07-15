//! Mutation rewriting for rows selected by a compiled scan.

use super::map_regex_runtime;
use crate::limits::Limits;
use crate::storage::{self, Candidate, RowLayout};
use crate::value::Value;
use crate::{Error, Result};

use super::super::ScanPlan;

pub(crate) fn rewrite_matching_rows<F>(
    candidate: &mut Candidate<'_>,
    plan: &ScanPlan,
    limits: &Limits,
    mut rewrite: F,
) -> Result<usize>
where
    F: FnMut(Vec<Value>) -> Result<Option<Vec<Value>>>,
{
    let layout = RowLayout {
        table: &plan.table,
        columns: &plan.schema,
    };
    let mut affected = 0_usize;
    let blob = candidate.source();

    for matched in plan.regex.find_iter(blob) {
        let matched = matched.map_err(|error| map_regex_runtime(error, limits))?;
        let values = storage::decode_row(matched.as_str(), layout)?;
        let replacement = rewrite(values)?;
        candidate.rewrite_row(
            matched.start()..matched.end(),
            layout,
            replacement.as_deref(),
        )?;
        affected = affected.checked_add(1).ok_or(Error::Capacity {
            operation: "counting affected rows",
        })?;
    }
    Ok(affected)
}
