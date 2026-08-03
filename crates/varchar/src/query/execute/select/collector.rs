//! Streaming and ordered `SELECT` row collection.

use std::cmp::Ordering;

use super::{ByteBudget, allocation_error};
use crate::output::{ColumnOrigin, ResultColumn, RowSet};
use crate::query::SelectPlan;
use crate::resolve::{ColumnLocation, ResolvedOrderTerm};
use crate::value::Value;
use crate::{Error, Result};

pub(super) struct RowCollector<'plan> {
    projection: &'plan [ColumnLocation],
    order_by: &'plan [ResolvedOrderTerm],
    limit: Option<u64>,
    offset: u64,
    retained: Option<usize>,
    output_budget: ByteBudget,
    row_structure: usize,
    state: CollectionState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CollectionStatus {
    Continue,
    Complete,
}

enum CollectionState {
    Complete,
    Streaming {
        rows: Vec<Vec<Value>>,
        skipped: u64,
        emitted: u64,
    },
    /// `rows` is a max-heap ordered by [`compare_pending`], so its root is the
    /// worst row still inside the retained pagination window.
    Ordered {
        rows: Vec<PendingRow>,
        next_ordinal: u64,
    },
}

struct PendingRow {
    projected: Vec<Value>,
    keys: Vec<Value>,
    ordinal: u64,
}

impl<'plan> RowCollector<'plan> {
    pub(super) fn new(plan: &'plan SelectPlan<'_, '_>, output_budget: ByteBudget) -> Result<Self> {
        let offset = plan.offset.unwrap_or(0);
        let retained = ordered_retention(offset, plan.limit);
        // `LIMIT 0` is the only window that can never gain a row, so it is also
        // the only one that skips every scan, join, and materialization step.
        let empty_window = retained == Some(0);
        let row_structure = if empty_window {
            0
        } else {
            row_structure_charge(plan.projection.len(), &output_budget)?
        };
        let state = if empty_window {
            CollectionState::Complete
        } else if plan.order_by.is_empty() {
            CollectionState::Streaming {
                rows: Vec::new(),
                skipped: 0,
                emitted: 0,
            }
        } else {
            CollectionState::Ordered {
                rows: Vec::new(),
                next_ordinal: 0,
            }
        };
        Ok(Self {
            projection: &plan.projection,
            order_by: &plan.order_by,
            limit: plan.limit,
            offset,
            retained,
            output_budget,
            row_structure,
            state,
        })
    }

    pub(super) fn should_scan(&self) -> bool {
        !matches!(self.state, CollectionState::Complete)
    }

    pub(super) fn collect(
        &mut self,
        sources: &[&[Value]],
        working_budget: &mut ByteBudget,
        transient_working_bytes: usize,
    ) -> Result<CollectionStatus> {
        match &mut self.state {
            CollectionState::Complete => Ok(CollectionStatus::Complete),
            CollectionState::Streaming {
                rows,
                skipped,
                emitted,
            } => collect_streaming(
                rows,
                skipped,
                emitted,
                self.limit,
                self.offset,
                self.projection,
                sources,
                self.row_structure,
                &mut self.output_budget,
            ),
            CollectionState::Ordered { rows, next_ordinal } => collect_ordered(
                rows,
                next_ordinal,
                self.projection,
                self.order_by,
                self.retained,
                sources,
                working_budget,
                transient_working_bytes,
            ),
        }
    }

    pub(super) fn finish(self, columns: Vec<ResultColumn>) -> Result<RowSet> {
        let Self {
            order_by,
            limit,
            offset,
            mut output_budget,
            row_structure,
            state,
            ..
        } = self;
        let rows = match state {
            CollectionState::Complete => Vec::new(),
            CollectionState::Streaming { rows, .. } => rows,
            CollectionState::Ordered { mut rows, .. } => {
                // `sort_unstable_by` is allocation-free. The monotonic ordinal is
                // the final key, so physical/nested-loop order wins every tie.
                // Bounded collection already dropped everything after the
                // window, so sorting the heap yields the window's own prefix.
                rows.sort_unstable_by(|left, right| compare_pending(left, right, order_by));
                materialize_ordered(rows, offset, limit, row_structure, &mut output_budget)?
            }
        };
        Ok(RowSet::new(columns, rows))
    }
}

pub(super) fn materialize_result_columns(
    plan: &SelectPlan<'_, '_>,
    output_budget: &mut ByteBudget,
) -> Result<Vec<ResultColumn>> {
    let column_slots = plan
        .projection
        .len()
        .checked_mul(std::mem::size_of::<ResultColumn>())
        .ok_or_else(|| output_budget.limit_error())?;
    output_budget.charge(column_slots)?;

    let mut columns = Vec::new();
    columns
        .try_reserve_exact(plan.projection.len())
        .map_err(|_| allocation_error("reserving query result columns"))?;
    for location in &plan.projection {
        let source = plan.sources[location.source];
        let column = &source.columns[location.column];
        output_budget.charge(column.name.len())?;
        let label = clone_string(&column.name, "cloning query output text")?;
        output_budget.charge(source.name.len())?;
        let table = clone_string(&source.name, "cloning query output text")?;
        output_budget.charge(column.name.len())?;
        let source_column = clone_string(&column.name, "cloning query output text")?;
        columns.push(ResultColumn::new(
            label,
            ColumnOrigin::new(table, source_column),
            column.data_type,
            column.nullable,
        ));
    }
    Ok(columns)
}

#[allow(clippy::too_many_arguments)]
fn collect_streaming(
    rows: &mut Vec<Vec<Value>>,
    skipped: &mut u64,
    emitted: &mut u64,
    limit: Option<u64>,
    offset: u64,
    projection: &[ColumnLocation],
    sources: &[&[Value]],
    row_structure: usize,
    output_budget: &mut ByteBudget,
) -> Result<CollectionStatus> {
    if *skipped < offset {
        *skipped = skipped.checked_add(1).ok_or(Error::Capacity {
            operation: "counting rows skipped by OFFSET",
        })?;
        return Ok(CollectionStatus::Continue);
    }
    if limit.is_some_and(|limit| *emitted >= limit) {
        return Ok(CollectionStatus::Complete);
    }
    let following_emitted = emitted.checked_add(1).ok_or(Error::Capacity {
        operation: "counting rows emitted through LIMIT",
    })?;

    let payload_bytes = projected_payload_size(projection, sources, output_budget)?;
    let row_charge = row_structure
        .checked_add(payload_bytes)
        .ok_or_else(|| output_budget.limit_error())?;
    output_budget.charge(row_charge)?;

    rows.try_reserve(1)
        .map_err(|_| allocation_error("reserving query result rows"))?;
    let mut row = Vec::new();
    row.try_reserve_exact(projection.len())
        .map_err(|_| allocation_error("reserving query result values"))?;
    for location in projection {
        row.push(clone_value(
            value_at(sources, *location),
            "cloning query output text",
        )?);
    }
    rows.push(row);
    *emitted = following_emitted;
    if limit == Some(*emitted) {
        Ok(CollectionStatus::Complete)
    } else {
        Ok(CollectionStatus::Continue)
    }
}

/// Retains one qualifying row for an ordered query, bounded by the pagination
/// window.
///
/// Once `retained` rows are held, the heap root is the worst row that could
/// still be returned. A candidate that does not beat it can never enter the
/// window, so it is rejected before anything is charged or cloned; a candidate
/// that does beat it evicts the root and refunds the root's working bytes.
#[allow(clippy::too_many_arguments)]
fn collect_ordered(
    rows: &mut Vec<PendingRow>,
    next_ordinal: &mut u64,
    projection: &[ColumnLocation],
    order_by: &[ResolvedOrderTerm],
    retained: Option<usize>,
    sources: &[&[Value]],
    working_budget: &mut ByteBudget,
    transient_working_bytes: usize,
) -> Result<CollectionStatus> {
    let ordinal = *next_ordinal;
    let following_ordinal = ordinal.checked_add(1).ok_or(Error::Capacity {
        operation: "assigning an ordered-row ordinal",
    })?;

    if let Some(retained) = retained {
        if retained == 0 {
            return Ok(CollectionStatus::Complete);
        }
        if rows.len() >= retained {
            let worst = rows.first().ok_or(Error::Capacity {
                operation: "reading the worst retained ordered row",
            })?;
            // The candidate's ordinal is always the largest so far, so a key
            // tie keeps the row already inside the window, exactly as the final
            // stable sort would.
            if compare_keys(
                order_by.iter().map(|term| value_at(sources, term.column)),
                ordinal,
                &worst.keys,
                worst.ordinal,
                order_by,
            ) != Ordering::Less
            {
                *next_ordinal = following_ordinal;
                return Ok(CollectionStatus::Continue);
            }
            let evicted = heap_pop(rows, order_by).ok_or(Error::Capacity {
                operation: "evicting an ordered row past the pagination window",
            })?;
            let refund = pending_row_charge(&evicted, working_budget)?;
            working_budget.release(refund);
        }
    }

    let projected_payload = payload_size(
        projection
            .iter()
            .map(|location| value_at(sources, *location)),
        working_budget,
    )?;
    let key_payload = payload_size(
        order_by.iter().map(|term| value_at(sources, term.column)),
        working_budget,
    )?;
    let charge = ordered_row_charge(
        projection.len(),
        order_by.len(),
        projected_payload,
        key_payload,
        working_budget,
    )?;
    working_budget.charge_with_transient(charge, transient_working_bytes)?;

    let mut projected = Vec::new();
    projected
        .try_reserve_exact(projection.len())
        .map_err(|_| allocation_error("reserving ordered projected values"))?;
    for location in projection {
        projected.push(clone_value(
            value_at(sources, *location),
            "cloning ordered projected text",
        )?);
    }

    let mut keys = Vec::new();
    keys.try_reserve_exact(order_by.len())
        .map_err(|_| allocation_error("reserving ORDER BY keys"))?;
    for term in order_by {
        keys.push(clone_value(
            value_at(sources, term.column),
            "cloning ORDER BY key text",
        )?);
    }

    heap_push(
        rows,
        PendingRow {
            projected,
            keys,
            ordinal,
        },
        order_by,
    )?;
    *next_ordinal = following_ordinal;
    Ok(CollectionStatus::Continue)
}

/// Pushes a row into the max-heap keyed by [`compare_pending`].
fn heap_push(
    rows: &mut Vec<PendingRow>,
    row: PendingRow,
    order_by: &[ResolvedOrderTerm],
) -> Result<()> {
    rows.try_reserve(1)
        .map_err(|_| allocation_error("retaining ordered query rows"))?;
    rows.push(row);
    let mut child = rows.len() - 1;
    while child > 0 {
        let parent = (child - 1) / 2;
        if compare_pending(&rows[child], &rows[parent], order_by) != Ordering::Greater {
            break;
        }
        rows.swap(child, parent);
        child = parent;
    }
    Ok(())
}

/// Removes the heap root, restoring the invariant. Never allocates.
fn heap_pop(rows: &mut Vec<PendingRow>, order_by: &[ResolvedOrderTerm]) -> Option<PendingRow> {
    let last = rows.len().checked_sub(1)?;
    rows.swap(0, last);
    let evicted = rows.pop();
    let mut parent = 0_usize;
    while let Some(left) = parent.checked_mul(2).and_then(|index| index.checked_add(1)) {
        let mut largest = parent;
        for child in [left, left.saturating_add(1)] {
            if child < rows.len()
                && compare_pending(&rows[child], &rows[largest], order_by) == Ordering::Greater
            {
                largest = child;
            }
        }
        if largest == parent {
            break;
        }
        rows.swap(parent, largest);
        parent = largest;
    }
    evicted
}

/// The pagination window can never need more than `OFFSET + LIMIT` rows.
///
/// `None` means the window is open-ended — no `LIMIT`, or a bound too large to
/// index a `Vec` — so every qualifying row has to be retained.
fn ordered_retention(offset: u64, limit: Option<u64>) -> Option<usize> {
    let limit = limit?;
    if limit == 0 {
        return Some(0);
    }
    usize::try_from(offset.checked_add(limit)?).ok()
}

/// Recomputes what `collect_ordered` charged for a retained row, so eviction
/// returns exactly those bytes to the working budget.
fn pending_row_charge(row: &PendingRow, budget: &ByteBudget) -> Result<usize> {
    ordered_row_charge(
        row.projected.len(),
        row.keys.len(),
        payload_size(&row.projected, budget)?,
        payload_size(&row.keys, budget)?,
        budget,
    )
}

fn materialize_ordered(
    pending: Vec<PendingRow>,
    offset: u64,
    limit: Option<u64>,
    row_structure: usize,
    output_budget: &mut ByteBudget,
) -> Result<Vec<Vec<Value>>> {
    let (skip, take) = ordered_window(pending.len(), offset, limit)?;
    let mut rows = Vec::new();
    for pending in pending.into_iter().skip(skip).take(take) {
        let payload = payload_size(pending.projected.iter(), output_budget)?;
        let charge = row_structure
            .checked_add(payload)
            .ok_or_else(|| output_budget.limit_error())?;
        output_budget.charge(charge)?;
        rows.try_reserve(1)
            .map_err(|_| allocation_error("reserving ordered query output rows"))?;
        rows.push(pending.projected);
    }
    Ok(rows)
}

fn ordered_window(cardinality: usize, offset: u64, limit: Option<u64>) -> Result<(usize, usize)> {
    let cardinality = u64::try_from(cardinality).map_err(|_| Error::Capacity {
        operation: "representing ordered query cardinality",
    })?;
    let skip = offset.min(cardinality);
    let remaining = cardinality - skip;
    let take = limit.unwrap_or(remaining).min(remaining);
    let skip = usize::try_from(skip).map_err(|_| Error::Capacity {
        operation: "applying ordered query OFFSET",
    })?;
    let take = usize::try_from(take).map_err(|_| Error::Capacity {
        operation: "applying ordered query LIMIT",
    })?;
    Ok((skip, take))
}

fn compare_pending(
    left: &PendingRow,
    right: &PendingRow,
    order_by: &[ResolvedOrderTerm],
) -> Ordering {
    compare_keys(
        &left.keys,
        left.ordinal,
        &right.keys,
        right.ordinal,
        order_by,
    )
}

/// Orders two rows by their sort keys and then by the monotonic ordinal.
///
/// Taking the keys as iterators lets a candidate still borrowed from the scan
/// be compared against a retained row without cloning either side.
fn compare_keys<'left, 'right>(
    left: impl IntoIterator<Item = &'left Value>,
    left_ordinal: u64,
    right: impl IntoIterator<Item = &'right Value>,
    right_ordinal: u64,
    order_by: &[ResolvedOrderTerm],
) -> Ordering {
    for ((left, right), term) in left.into_iter().zip(right).zip(order_by) {
        let ordering = compare_values(left, right, term.descending);
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left_ordinal.cmp(&right_ordinal)
}

fn compare_values(left: &Value, right: &Value, descending: bool) -> Ordering {
    let ascending = match (left, right) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Greater,
        (_, Value::Null) => Ordering::Less,
        (Value::Integer(left), Value::Integer(right)) => left.cmp(right),
        (Value::Boolean(left), Value::Boolean(right)) => left.cmp(right),
        (Value::Text(left), Value::Text(right)) => left.chars().cmp(right.chars()),
        _ => unreachable!("resolved ORDER BY keys always have one scalar type"),
    };
    if descending {
        ascending.reverse()
    } else {
        ascending
    }
}

fn ordered_row_charge(
    projection_count: usize,
    key_count: usize,
    projected_payload: usize,
    key_payload: usize,
    budget: &ByteBudget,
) -> Result<usize> {
    // One descriptor is live per row and three more conservatively account for
    // geometric growth of the pending-row vector. `PendingRow` contains both
    // child-vector descriptors and the charged `u64` ordinal.
    let pending_descriptors = std::mem::size_of::<PendingRow>()
        .checked_mul(4)
        .ok_or_else(|| budget.limit_error())?;
    let projected_slots = projection_count
        .checked_mul(std::mem::size_of::<Value>())
        .ok_or_else(|| budget.limit_error())?;
    let key_slots = key_count
        .checked_mul(std::mem::size_of::<Value>())
        .ok_or_else(|| budget.limit_error())?;
    pending_descriptors
        .checked_add(projected_slots)
        .and_then(|charge| charge.checked_add(projected_payload))
        .and_then(|charge| charge.checked_add(key_slots))
        .and_then(|charge| charge.checked_add(key_payload))
        .ok_or_else(|| budget.limit_error())
}

fn row_structure_charge(column_count: usize, budget: &ByteBudget) -> Result<usize> {
    let value_slots = column_count
        .checked_mul(std::mem::size_of::<Value>())
        .ok_or_else(|| budget.limit_error())?;
    // `Vec::try_reserve(1)` may grow the outer row vector geometrically. Keep
    // four row descriptors in the logical charge to account conservatively.
    let row_descriptors = std::mem::size_of::<Vec<Value>>()
        .checked_mul(4)
        .ok_or_else(|| budget.limit_error())?;
    row_descriptors
        .checked_add(value_slots)
        .ok_or_else(|| budget.limit_error())
}

fn projected_payload_size(
    projection: &[ColumnLocation],
    sources: &[&[Value]],
    budget: &ByteBudget,
) -> Result<usize> {
    payload_size(
        projection
            .iter()
            .map(|location| value_at(sources, *location)),
        budget,
    )
}

fn payload_size<'value>(
    values: impl IntoIterator<Item = &'value Value>,
    budget: &ByteBudget,
) -> Result<usize> {
    values.into_iter().try_fold(0_usize, |total, value| {
        total
            .checked_add(value_payload_size(value))
            .ok_or_else(|| budget.limit_error())
    })
}

fn value_at<'value>(sources: &[&'value [Value]], location: ColumnLocation) -> &'value Value {
    &sources[location.source][location.column]
}

fn value_payload_size(value: &Value) -> usize {
    match value {
        Value::Text(value) => value.len(),
        Value::Integer(_) | Value::Boolean(_) | Value::Null => 0,
    }
}

fn clone_value(value: &Value, operation: &'static str) -> Result<Value> {
    match value {
        Value::Text(value) => Ok(Value::Text(clone_string(value, operation)?)),
        Value::Integer(value) => Ok(Value::Integer(*value)),
        Value::Boolean(value) => Ok(Value::Boolean(*value)),
        Value::Null => Ok(Value::Null),
    }
}

fn clone_string(value: &str, operation: &'static str) -> Result<String> {
    let mut cloned = String::new();
    cloned
        .try_reserve_exact(value.len())
        .map_err(|_| allocation_error(operation))?;
    cloned.push_str(value);
    Ok(cloned)
}

#[cfg(test)]
mod tests;
