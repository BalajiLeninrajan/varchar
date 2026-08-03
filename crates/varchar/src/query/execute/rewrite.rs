//! Mutation rewriting for rows selected by a compiled scan.

use super::map_regex_runtime;
use crate::expression::Evaluator;
use crate::limits::Limits;
use crate::storage::{self, Candidate};
use crate::value::Value;
use crate::{Error, Result};

use super::super::ScanPlan;

pub(crate) fn rewrite_matching_rows<F>(
    candidate: &mut Candidate<'_>,
    plan: &ScanPlan<'_>,
    limits: &Limits,
    mut rewrite: F,
) -> Result<usize>
where
    F: FnMut(Vec<Value>) -> Result<Option<Vec<Value>>>,
{
    let layout = plan.row_layout();
    let mut affected = 0_usize;
    let blob = candidate.source();
    let mut evaluator = plan
        .local_residual
        .as_ref()
        .map(|program| Evaluator::new(program, limits.regex_backtrack_limit))
        .transpose()?;

    for matched in plan.regex.find_iter(blob) {
        let matched = matched.map_err(|error| map_regex_runtime(error, limits))?;
        let values = storage::decode_row(matched.as_str(), layout)?;
        if let (Some(program), Some(evaluator)) = (&plan.local_residual, &mut evaluator)
            && !evaluator.evaluate_where_local(program, 0, &values)?
        {
            continue;
        }
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
