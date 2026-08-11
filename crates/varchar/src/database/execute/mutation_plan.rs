//! Statement-wide planning for direct `UPDATE` and `DELETE` mutations.
//!
//! Root targets are frozen from one scan of the original validated blob. The
//! planner then sorts immutable source ranges, installs direct overlays, encodes
//! replacements, and only afterward hands physical edits to storage.

mod model;
mod referential;

use std::ops::Range;

use model::{FrozenRow, PreparedDirectUpdate, RowIdentity, WorkingBudget, decoded_values_bytes};
use referential::{ReferentialAction, ReferentialIndex};

use crate::expression::Evaluator;
use crate::limits::{Limits, check_limit};
use crate::query::{self, ScanPlan};
use crate::storage::{self, Candidate, Catalog, RowLayout, with_validated_row_encoder};
#[cfg(test)]
use crate::storage::{MeasuredRowEncoding, ValidatedRowEncoder};
use crate::{Error, Resource, Result, Value};

pub(super) struct MutationPlan {
    rows: Vec<FrozenRow>,
    direct_affected: usize,
}

impl MutationPlan {
    pub(super) fn update(
        candidate: &mut Candidate<'_>,
        scan: &ScanPlan<'_>,
        limits: &Limits,
        assignments: &mut [(usize, Value)],
        direct_auto_increment: Option<i64>,
    ) -> Result<Self> {
        let initial_sequence_working_bytes = candidate.deferred_auto_increment_working_bytes();
        let blob = candidate.source();
        let (mut rows, direct_affected, mut budget) =
            freeze_direct_targets(blob, scan, limits, initial_sequence_working_bytes)?;
        sort_and_validate_ranges(&mut rows)?;
        if rows.is_empty() {
            return Ok(Self {
                rows,
                direct_affected,
            });
        }
        if let Some(last) = direct_auto_increment {
            defer_auto_increment(candidate, scan.row_layout().table, last, &mut budget)?;
        }

        let layout = scan.validated_row_layout();
        let update =
            PreparedDirectUpdate::new(assignments, layout.column_count(), rows[0].identity())?;
        for row in &mut rows {
            row.install_direct_update(&update, &mut budget)?;
        }

        let parent_schema = candidate
            .catalog()
            .table(scan.row_layout().table)
            .expect("a compiled mutation scan names a catalog table");
        if let Some(primary_key) = parent_schema.primary_key {
            let mut update_queue = Vec::new();
            let mut queue_working_bytes = 0;
            for (index, row) in rows.iter_mut().enumerate() {
                if row.mark_update_queued(primary_key) {
                    push_update_queue(
                        &mut update_queue,
                        index,
                        &mut queue_working_bytes,
                        &mut budget,
                    )?;
                }
            }
            if !update_queue.is_empty() {
                let referential = ReferentialIndex::build(
                    blob,
                    candidate.catalog(),
                    scan.row_layout().table,
                    &rows,
                    ReferentialAction::Update,
                    &mut budget,
                )?;
                referential.initialize_direct_rows(&rows)?;
                referential.expand_update_actions(
                    &mut rows,
                    &mut update_queue,
                    &mut queue_working_bytes,
                    &mut budget,
                )?;
                referential.release(&mut budget);
            }
            drop(update_queue);
            budget.release(queue_working_bytes);
        }

        sort_and_validate_ranges(&mut rows)?;
        defer_induced_auto_increments(candidate, blob, &rows, &mut budget)?;
        let sequence_edit_lengths = candidate.deferred_auto_increment_lengths()?;
        encode_and_check_updates(
            candidate.catalog(),
            blob,
            limits,
            &mut rows,
            sequence_edit_lengths,
            &mut budget,
        )?;
        if let Some(sequence_replacement_bytes) =
            candidate.deferred_auto_increment_max_replacement_bytes()?
        {
            budget.check_transient(sequence_replacement_bytes)?;
        }

        Ok(Self {
            rows,
            direct_affected,
        })
    }

    pub(super) fn delete(
        blob: &str,
        catalog: &Catalog,
        scan: &ScanPlan<'_>,
        limits: &Limits,
    ) -> Result<Self> {
        let (mut rows, direct_affected, mut budget) = freeze_direct_targets(blob, scan, limits, 0)?;
        sort_and_validate_ranges(&mut rows)?;
        if rows.is_empty() {
            return Ok(Self {
                rows,
                direct_affected,
            });
        }

        let referential = ReferentialIndex::build(
            blob,
            catalog,
            scan.row_layout().table,
            &rows,
            ReferentialAction::Delete,
            &mut budget,
        )?;
        referential.initialize_direct_rows(&rows)?;
        let mut delete_queue = Vec::new();
        let mut queue_working_bytes = 0;
        for row in &mut rows {
            if row.request_delete(&mut budget)? {
                push_delete_queue(
                    &mut delete_queue,
                    row.identity(),
                    &mut queue_working_bytes,
                    &mut budget,
                )?;
            }
        }
        referential.expand_delete_actions(
            &mut rows,
            &mut delete_queue,
            &mut queue_working_bytes,
            &mut budget,
        )?;
        drop(delete_queue);
        budget.release(queue_working_bytes);
        referential.release(&mut budget);

        for row in &mut rows {
            if !row.needs_set_null() {
                continue;
            }
            let identity = row.identity();
            let record = blob
                .get(identity.range())
                .ok_or_else(|| invalid_range(identity.start()))?;
            let row_record = storage::row_record(record, identity.start())?;
            let table = catalog.validated_table(row_record.table()).ok_or_else(|| {
                Error::CorruptStorage {
                    offset: identity.start(),
                    message: String::from("planned mutation row references an unknown table"),
                }
            })?;
            with_validated_row_encoder(table.validated_row_layout(), |encoder| {
                row.encode_set_null(&encoder, &mut budget)
            })?;
        }

        sort_and_validate_ranges(&mut rows)?;
        let mut projected = blob.len();
        for row in &rows {
            let replacement = row.replacement()?;
            projected = replace_projected_bytes(
                projected,
                row.identity().len(),
                replacement.map_or(0, str::len),
                limits.max_database_bytes,
            )?;
        }
        check_limit(
            projected,
            limits.max_database_bytes,
            Resource::DatabaseBytes,
        )?;

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

fn push_update_queue(
    queue: &mut Vec<usize>,
    frozen_index: usize,
    queue_working_bytes: &mut usize,
    budget: &mut WorkingBudget,
) -> Result<()> {
    let charged =
        budget.reserve_for_push_charged(queue, "reserving the referential update queue")?;
    *queue_working_bytes = queue_working_bytes
        .checked_add(charged)
        .ok_or_else(|| budget.limit_error())?;
    queue.push(frozen_index);
    Ok(())
}

fn push_delete_queue(
    queue: &mut Vec<RowIdentity>,
    identity: RowIdentity,
    queue_working_bytes: &mut usize,
    budget: &mut WorkingBudget,
) -> Result<()> {
    let charged =
        budget.reserve_for_push_charged(queue, "reserving the referential delete queue")?;
    *queue_working_bytes = queue_working_bytes
        .checked_add(charged)
        .ok_or_else(|| budget.limit_error())?;
    queue.push(identity);
    Ok(())
}

fn row_record_for_identity<'a>(
    blob: &'a str,
    identity: RowIdentity,
) -> Result<storage::RowRecordRef<'a>> {
    let record = blob
        .get(identity.range())
        .ok_or_else(|| invalid_range(identity.start()))?;
    let row_record = storage::row_record(record, identity.start())?;
    if row_record.range() != identity.range() {
        return Err(invalid_range(identity.start()));
    }
    Ok(row_record)
}

#[cfg(test)]
fn sequence_edit_lengths_for_targets(
    target_count: usize,
    lengths: impl FnOnce() -> Result<Option<(usize, usize)>>,
) -> Result<Option<(usize, usize)>> {
    if target_count == 0 {
        Ok(None)
    } else {
        lengths()
    }
}

fn freeze_direct_targets(
    blob: &str,
    scan: &ScanPlan<'_>,
    limits: &Limits,
    retained_working_bytes: usize,
) -> Result<(Vec<FrozenRow>, usize, WorkingBudget)> {
    let mut budget = WorkingBudget::for_database_limit(limits.max_database_bytes);
    budget.charge(retained_working_bytes)?;
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
    Ok((rows, direct_affected, budget))
}

#[cfg(test)]
fn measure_and_check_update_database_size<'brand>(
    source_bytes: usize,
    limit: usize,
    rows: &mut [FrozenRow],
    encoder: &ValidatedRowEncoder<'_, 'brand>,
    update: &PreparedDirectUpdate<'_>,
    sequence_edit_lengths: Option<(usize, usize)>,
    budget: &mut WorkingBudget,
) -> Result<(Vec<MeasuredRowEncoding<'brand>>, usize)> {
    let mut measurements = Vec::new();
    let measurement_working_bytes = budget.reserve_exact(
        &mut measurements,
        rows.len(),
        "reserving measured row encodings",
    )?;
    let mut projected = source_bytes;
    if let Some((original, replacement)) = sequence_edit_lengths {
        projected = replace_projected_bytes(projected, original, replacement, limit)?;
    }
    for row in rows {
        let measured = row.measure_direct_update(update, encoder)?;
        projected = replace_projected_bytes(
            projected,
            row.identity().len(),
            measured.encoded_len(),
            limit,
        )?;
        measurements.push(measured);
    }
    check_limit(projected, limit, Resource::DatabaseBytes)?;
    Ok((measurements, measurement_working_bytes))
}

fn defer_auto_increment(
    candidate: &mut Candidate<'_>,
    table: &str,
    last: i64,
    budget: &mut WorkingBudget,
) -> Result<()> {
    let reservation_bytes = candidate.deferred_auto_increment_reservation_bytes()?;
    budget.charge(reservation_bytes)?;
    if let Err(error) = candidate.defer_auto_increment(table, last) {
        budget.release(reservation_bytes);
        return Err(error);
    }
    Ok(())
}

fn defer_induced_auto_increments(
    candidate: &mut Candidate<'_>,
    blob: &str,
    rows: &[FrozenRow],
    budget: &mut WorkingBudget,
) -> Result<()> {
    for row in rows {
        if !row.needs_update() {
            continue;
        }
        let record = row_record_for_identity(blob, row.identity())?;
        let Some(auto_increment) = candidate.catalog().auto_increment(record.table()) else {
            continue;
        };
        let Some(Value::Integer(value)) = row.effective_value(auto_increment.column) else {
            continue;
        };
        if *value > auto_increment.last {
            defer_auto_increment(candidate, record.table(), *value, budget)?;
        }
    }
    Ok(())
}

fn encode_and_check_updates(
    catalog: &Catalog,
    blob: &str,
    limits: &Limits,
    rows: &mut [FrozenRow],
    sequence_edit_lengths: Option<(usize, usize)>,
    budget: &mut WorkingBudget,
) -> Result<()> {
    let mut projected = blob.len();
    if let Some((original, replacement)) = sequence_edit_lengths {
        projected =
            replace_projected_bytes(projected, original, replacement, limits.max_database_bytes)?;
    }
    for row in rows.iter() {
        let record = row_record_for_identity(blob, row.identity())?;
        let table =
            catalog
                .validated_table(record.table())
                .ok_or_else(|| Error::CorruptStorage {
                    offset: row.identity().start(),
                    message: String::from("planned mutation row references an unknown table"),
                })?;
        let encoded_len = with_validated_row_encoder(table.validated_row_layout(), |encoder| {
            row.measure_effective_update(&encoder)
                .map(|measured| measured.encoded_len())
        })?;
        projected = replace_projected_bytes(
            projected,
            row.identity().len(),
            encoded_len,
            limits.max_database_bytes,
        )?;
    }
    check_limit(
        projected,
        limits.max_database_bytes,
        Resource::DatabaseBytes,
    )?;

    for row in rows {
        let record = row_record_for_identity(blob, row.identity())?;
        let table =
            catalog
                .validated_table(record.table())
                .ok_or_else(|| Error::CorruptStorage {
                    offset: row.identity().start(),
                    message: String::from("planned mutation row references an unknown table"),
                })?;
        with_validated_row_encoder(table.validated_row_layout(), |encoder| {
            let measured = row.measure_effective_update(&encoder)?;
            row.encode_effective_update(&encoder, measured, budget)
        })?;
    }
    Ok(())
}

fn replace_projected_bytes(
    current: usize,
    original: usize,
    replacement: usize,
    limit: usize,
) -> Result<usize> {
    current
        .checked_sub(original)
        .and_then(|bytes| bytes.checked_add(replacement))
        .ok_or(Error::ResourceLimit {
            resource: Resource::DatabaseBytes,
            limit,
        })
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
