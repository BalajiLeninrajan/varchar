//! Statement-wide planning for direct `DELETE` mutations.
//!
//! Root targets are frozen from one scan of the original validated blob. The
//! planner then sorts immutable source ranges and only afterward hands physical
//! edits to storage.

mod model;

use std::ops::Range;

use model::{FrozenRow, RowIdentity, WorkingBudget, decoded_values_bytes};

use crate::expression::Evaluator;
use crate::limits::Limits;
use crate::query::{self, ScanPlan};
use crate::storage::{self, Candidate, RowLayout};
use crate::{Error, Result, Value};

pub(super) struct MutationPlan {
    rows: Vec<FrozenRow>,
    direct_affected: usize,
}

impl MutationPlan {
    pub(super) fn delete(blob: &str, scan: &ScanPlan<'_>, limits: &Limits) -> Result<Self> {
        let (mut rows, direct_affected) = freeze_direct_targets(blob, scan, limits)?;
        sort_and_validate_ranges(&mut rows)?;
        for row in &mut rows {
            row.mark_direct_delete()?;
        }
        Ok(Self {
            rows,
            direct_affected,
        })
    }

    pub(super) fn apply(self, candidate: &mut Candidate<'_>) -> Result<usize> {
        let direct_affected = self.direct_affected;
        for row in self.rows {
            let identity = row.identity();
            candidate.rewrite_encoded_row(identity.range(), row.replacement()?)?;
        }
        Ok(direct_affected)
    }
}

fn freeze_direct_targets(
    blob: &str,
    scan: &ScanPlan<'_>,
    limits: &Limits,
) -> Result<(Vec<FrozenRow>, usize)> {
    let mut budget = WorkingBudget::for_database_limit(limits.max_database_bytes);
    let residual = scan.local_residual();
    let evaluator_bytes = residual
        .map(Evaluator::working_bytes)
        .transpose()?
        .unwrap_or(0);
    budget.charge(evaluator_bytes)?;
    let mut evaluator = residual
        .map(|program| Evaluator::new(program, limits.regex_backtrack_limit))
        .transpose()?;

    let ranges = scan.regex().find_iter(blob).map(|matched| {
        let matched = matched.map_err(|error| query::map_regex_runtime(error, limits))?;
        Ok(matched.start()..matched.end())
    });
    let (rows, direct_affected) =
        freeze_rows(blob, ranges, scan.row_layout(), &mut budget, |values| {
            if let (Some(program), Some(evaluator)) = (residual, &mut evaluator) {
                evaluator.evaluate_where_local(program, 0, values)
            } else {
                Ok(true)
            }
        })?;
    drop(evaluator);
    budget.release(evaluator_bytes);
    Ok((rows, direct_affected))
}

fn freeze_rows(
    blob: &str,
    ranges: impl IntoIterator<Item = Result<Range<usize>>>,
    layout: RowLayout<'_>,
    budget: &mut WorkingBudget,
    mut passes_where: impl FnMut(&[Value]) -> Result<bool>,
) -> Result<(Vec<FrozenRow>, usize)> {
    let mut rows = Vec::new();
    let mut direct_affected = 0_usize;

    for range in ranges {
        let range = range?;
        let identity = RowIdentity::new(range.clone())?;
        let record = blob
            .get(range.clone())
            .ok_or_else(|| invalid_range(range.start))?;
        let row_record = storage::row_record(record, range.start)?;
        if row_record.range() != range {
            return Err(invalid_range(range.start));
        }

        let decoded_bytes = decoded_values_bytes(layout.columns.len(), range.len(), budget)?;
        budget.check_transient(decoded_bytes)?;
        let values = storage::decode_row(record, layout)?;
        if !passes_where(&values)? {
            continue;
        }

        budget.charge(decoded_bytes)?;
        budget.reserve_for_push(&mut rows, "reserving frozen mutation targets")?;
        rows.push(FrozenRow::new(identity, values));
        direct_affected = direct_affected.checked_add(1).ok_or(Error::Capacity {
            operation: "counting affected rows",
        })?;
    }

    Ok((rows, direct_affected))
}

fn sort_and_validate_ranges(rows: &mut [FrozenRow]) -> Result<()> {
    rows.sort_unstable_by_key(|row| row.identity().start());
    for adjacent in rows.windows(2) {
        let previous = adjacent[0].identity();
        let next = adjacent[1].identity();
        if previous.overlaps(next) {
            return Err(Error::CorruptStorage {
                offset: next.start(),
                message: String::from("planned mutation row ranges overlap"),
            });
        }
    }
    Ok(())
}

fn invalid_range(offset: usize) -> Error {
    Error::CorruptStorage {
        offset,
        message: String::from("planned mutation row range is outside the database"),
    }
}

#[cfg(test)]
mod tests;
