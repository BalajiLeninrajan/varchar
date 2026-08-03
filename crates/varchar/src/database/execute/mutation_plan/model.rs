//! Frozen row identities, effective overlays, and mutation working memory.

use std::cell::Cell;
use std::mem::size_of;
use std::ops::Range;

use crate::limits::{check_limit, storage_working_limit};
use crate::storage::{MeasuredRowEncoding, ValidatedRowEncoder};
use crate::{Error, Resource, Result, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RowIdentity {
    start: usize,
    end: usize,
}

impl RowIdentity {
    pub(super) fn new(range: Range<usize>) -> Result<Self> {
        if range.start >= range.end {
            return Err(invalid_identity(range.start));
        }
        Ok(Self {
            start: range.start,
            end: range.end,
        })
    }

    pub(super) const fn start(self) -> usize {
        self.start
    }

    #[cfg(test)]
    pub(super) const fn end(self) -> usize {
        self.end
    }

    pub(super) const fn overlaps(self, next: Self) -> bool {
        self.end > next.start
    }

    pub(super) const fn len(self) -> usize {
        self.end - self.start
    }

    pub(super) const fn range(self) -> Range<usize> {
        self.start..self.end
    }
}

#[derive(Debug)]
struct DirectOverlay {
    column: usize,
    value: Value,
}

pub(super) struct PreparedDirectUpdate<'assignments> {
    assignments: &'assignments [(usize, Value)],
}

impl<'assignments> PreparedDirectUpdate<'assignments> {
    pub(super) fn new(
        assignments: &'assignments mut [(usize, Value)],
        column_count: usize,
        identity: RowIdentity,
    ) -> Result<Self> {
        assignments.sort_unstable_by_key(|(column, _)| *column);
        for (position, (column, _)) in assignments.iter().enumerate() {
            if *column >= column_count {
                return Err(Error::Schema(format!(
                    "UPDATE assignment column {column} is outside a frozen row"
                )));
            }
            if position > 0 && assignments[position - 1].0 == *column {
                return Err(direct_conflict(identity));
            }
        }
        Ok(Self { assignments })
    }

    pub(super) fn assignments(&self) -> &[(usize, Value)] {
        self.assignments
    }

    fn value_at<'values>(
        &'values self,
        original_values: &'values [Value],
        column: usize,
        next_assignment: &Cell<usize>,
    ) -> Option<&'values Value> {
        let position = next_assignment.get();
        if let Some((assigned, value)) = self.assignments.get(position) {
            if *assigned == column {
                next_assignment.set(position + 1);
                return Some(value);
            }
            debug_assert!(*assigned > column);
        }
        original_values.get(column)
    }
}

#[derive(Debug)]
enum MutationState {
    Fresh,
    MeasuredUpdate,
    InstalledUpdate {
        direct_overlays: Vec<DirectOverlay>,
        overlay_working_bytes: usize,
    },
    EncodedUpdate(String),
    Deleted,
}

#[derive(Debug)]
pub(super) struct FrozenRow {
    identity: RowIdentity,
    original_values: Vec<Value>,
    mutation: MutationState,
}

impl FrozenRow {
    pub(super) fn new(identity: RowIdentity, original_values: Vec<Value>) -> Self {
        Self {
            identity,
            original_values,
            mutation: MutationState::Fresh,
        }
    }

    pub(super) const fn identity(&self) -> RowIdentity {
        self.identity
    }

    #[cfg(test)]
    pub(super) fn original_values(&self) -> &[Value] {
        &self.original_values
    }

    pub(super) fn original_value(&self, column: usize) -> Option<&Value> {
        self.original_values.get(column)
    }

    pub(super) fn measure_direct_update<'brand>(
        &mut self,
        update: &PreparedDirectUpdate<'_>,
        encoder: &ValidatedRowEncoder<'_, 'brand>,
    ) -> Result<MeasuredRowEncoding<'brand>> {
        if !matches!(self.mutation, MutationState::Fresh) {
            return Err(direct_conflict(self.identity));
        }
        let next_assignment = Cell::new(0);
        let measured = encoder.measure(self.original_values.len(), |column| {
            update.value_at(&self.original_values, column, &next_assignment)
        })?;
        self.mutation = MutationState::MeasuredUpdate;
        Ok(measured)
    }

    pub(super) fn install_direct_update(
        &mut self,
        update: &PreparedDirectUpdate<'_>,
        budget: &mut WorkingBudget,
    ) -> Result<()> {
        if !matches!(self.mutation, MutationState::MeasuredUpdate) {
            return Err(direct_conflict(self.identity));
        }

        let assignments = update.assignments();
        let payload_bytes = assignments.iter().try_fold(0_usize, |total, (_, value)| {
            total
                .checked_add(value_payload_bytes(value))
                .ok_or_else(|| budget.limit_error())
        })?;
        let mut direct_overlays = Vec::new();
        let descriptor_bytes = budget.reserve_exact(
            &mut direct_overlays,
            assignments.len(),
            "reserving direct mutation overlays",
        )?;
        if let Err(error) = budget.charge(payload_bytes) {
            drop(direct_overlays);
            budget.release(descriptor_bytes);
            return Err(error);
        }

        for (column, value) in assignments {
            let value = match clone_value(value) {
                Ok(value) => value,
                Err(error) => {
                    drop(direct_overlays);
                    budget.release(payload_bytes);
                    budget.release(descriptor_bytes);
                    return Err(error);
                }
            };
            direct_overlays.push(DirectOverlay {
                column: *column,
                value,
            });
        }
        let overlay_working_bytes = descriptor_bytes
            .checked_add(payload_bytes)
            .expect("successful storage-working charges fit in usize");
        self.mutation = MutationState::InstalledUpdate {
            direct_overlays,
            overlay_working_bytes,
        };
        Ok(())
    }

    pub(super) fn mark_direct_delete(&mut self) -> Result<()> {
        if !matches!(self.mutation, MutationState::Fresh) {
            return Err(direct_conflict(self.identity));
        }
        self.mutation = MutationState::Deleted;
        Ok(())
    }

    pub(super) fn encode_effective_update<'brand>(
        &mut self,
        encoder: &ValidatedRowEncoder<'_, 'brand>,
        measured: MeasuredRowEncoding<'brand>,
        budget: &mut WorkingBudget,
    ) -> Result<()> {
        let (direct_overlays, overlay_working_bytes) = match &self.mutation {
            MutationState::InstalledUpdate {
                direct_overlays,
                overlay_working_bytes,
            } => (direct_overlays, *overlay_working_bytes),
            _ => return Err(direct_conflict(self.identity)),
        };
        let encoded_len = measured.encoded_len();
        budget.charge(encoded_len)?;
        let next_overlay = Cell::new(0);
        let encoded = match encoder.encode(measured, |column| {
            effective_value(
                &self.original_values,
                direct_overlays,
                column,
                &next_overlay,
            )
        }) {
            Ok(encoded) => encoded,
            Err(error) => {
                budget.release(encoded_len);
                return Err(error);
            }
        };
        let installed =
            std::mem::replace(&mut self.mutation, MutationState::EncodedUpdate(encoded));
        drop(installed);
        budget.release(overlay_working_bytes);
        Ok(())
    }

    pub(super) fn replacement(&self) -> Result<Option<&str>> {
        match &self.mutation {
            MutationState::EncodedUpdate(encoded) => Ok(Some(encoded)),
            MutationState::Deleted => Ok(None),
            MutationState::Fresh
            | MutationState::MeasuredUpdate
            | MutationState::InstalledUpdate { .. } => Err(Error::Capacity {
                operation: "reading a planned row replacement",
            }),
        }
    }
}

fn effective_value<'values>(
    original_values: &'values [Value],
    direct_overlays: &'values [DirectOverlay],
    column: usize,
    next_overlay: &Cell<usize>,
) -> Option<&'values Value> {
    let position = next_overlay.get();
    if let Some(overlay) = direct_overlays.get(position) {
        if overlay.column == column {
            next_overlay.set(position + 1);
            return Some(&overlay.value);
        }
        debug_assert!(overlay.column > column);
    }
    original_values.get(column)
}

pub(super) struct WorkingBudget {
    used: usize,
    limit: usize,
}

impl WorkingBudget {
    pub(super) const fn for_database_limit(max_database_bytes: usize) -> Self {
        Self {
            used: 0,
            limit: storage_working_limit(max_database_bytes),
        }
    }

    #[cfg(test)]
    pub(super) const fn with_limit(limit: usize) -> Self {
        Self { used: 0, limit }
    }

    pub(super) fn charge(&mut self, amount: usize) -> Result<()> {
        let next = self
            .used
            .checked_add(amount)
            .ok_or_else(|| self.limit_error())?;
        check_limit(next, self.limit, Resource::StorageWorkingBytes)?;
        self.used = next;
        Ok(())
    }

    pub(super) fn check_transient(&self, amount: usize) -> Result<()> {
        let peak = self
            .used
            .checked_add(amount)
            .ok_or_else(|| self.limit_error())?;
        check_limit(peak, self.limit, Resource::StorageWorkingBytes)
    }

    pub(super) fn release(&mut self, amount: usize) {
        self.used = self
            .used
            .checked_sub(amount)
            .expect("only a live storage-working charge can be released");
    }

    pub(super) fn reserve_for_push<T>(
        &mut self,
        values: &mut Vec<T>,
        operation: &'static str,
    ) -> Result<()> {
        let _ = self.reserve_for_push_charged(values, operation)?;
        Ok(())
    }

    pub(super) fn reserve_for_push_charged<T>(
        &mut self,
        values: &mut Vec<T>,
        operation: &'static str,
    ) -> Result<usize> {
        if values.len() < values.capacity() {
            return Ok(0);
        }
        let target_capacity = if values.capacity() == 0 {
            1
        } else {
            values
                .capacity()
                .checked_mul(2)
                .ok_or_else(|| self.limit_error())?
        };
        let additional = target_capacity
            .checked_sub(values.len())
            .ok_or_else(|| self.limit_error())?;
        self.reserve_exact(values, additional, operation)
    }

    pub(super) fn reserve_exact<T>(
        &mut self,
        values: &mut Vec<T>,
        additional: usize,
        operation: &'static str,
    ) -> Result<usize> {
        let bytes = additional
            .checked_mul(size_of::<T>())
            .ok_or_else(|| self.limit_error())?;
        self.charge(bytes)?;
        if values.try_reserve_exact(additional).is_err() {
            self.release(bytes);
            return Err(Error::Allocation { operation });
        }
        Ok(bytes)
    }

    pub(super) const fn limit_error(&self) -> Error {
        Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit: self.limit,
        }
    }

    #[cfg(test)]
    pub(super) const fn used(&self) -> usize {
        self.used
    }
}

pub(super) fn decoded_values_bytes(
    column_count: usize,
    encoded_row_bytes: usize,
    budget: &WorkingBudget,
) -> Result<usize> {
    column_count
        .checked_mul(size_of::<Value>())
        .and_then(|slots| slots.checked_add(encoded_row_bytes))
        .ok_or_else(|| budget.limit_error())
}

#[cfg(test)]
std::thread_local! {
    static VALUE_CLONE_FAILURE_AFTER: Cell<Option<usize>> = const {
        Cell::new(None)
    };
}

#[cfg(test)]
pub(super) fn set_value_clone_failure_after(successful_clones: Option<usize>) {
    VALUE_CLONE_FAILURE_AFTER.with(|remaining| remaining.set(successful_clones));
}

fn clone_value(value: &Value) -> Result<Value> {
    #[cfg(test)]
    if VALUE_CLONE_FAILURE_AFTER.with(|remaining| match remaining.get() {
        Some(0) => {
            remaining.set(None);
            true
        }
        Some(count) => {
            remaining.set(Some(count - 1));
            false
        }
        None => false,
    }) {
        return Err(Error::Allocation {
            operation: "cloning a direct mutation value",
        });
    }

    match value {
        Value::Text(value) => {
            let mut cloned = String::new();
            cloned
                .try_reserve_exact(value.len())
                .map_err(|_| Error::Allocation {
                    operation: "cloning a direct mutation TEXT value",
                })?;
            cloned.push_str(value);
            Ok(Value::Text(cloned))
        }
        Value::Integer(value) => Ok(Value::Integer(*value)),
        Value::Boolean(value) => Ok(Value::Boolean(*value)),
        Value::Null => Ok(Value::Null),
    }
}

const fn value_payload_bytes(value: &Value) -> usize {
    match value {
        Value::Text(value) => value.len(),
        Value::Integer(_) | Value::Boolean(_) | Value::Null => 0,
    }
}

fn direct_conflict(identity: RowIdentity) -> Error {
    Error::Constraint(format!(
        "conflicting direct mutations target the row at byte {}",
        identity.start()
    ))
}

fn invalid_identity(offset: usize) -> Error {
    Error::CorruptStorage {
        offset,
        message: String::from("mutation row identity is empty or reversed"),
    }
}
