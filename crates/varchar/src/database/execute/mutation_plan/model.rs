//! Frozen row identities, effective overlays, and mutation working memory.

use std::cell::Cell;
use std::mem::size_of;
use std::ops::Range;

use crate::limits::ByteBudget;
use crate::storage::{MeasuredRowEncoding, ValidatedRowEncoder};
use crate::{Error, Result, Value};

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
struct UpdateOverlay {
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

    #[cfg(test)]
    fn value_at<'values>(
        &'values self,
        original_values: &'values [Value],
        column: usize,
    ) -> Option<&'values Value> {
        self.assignments
            .binary_search_by_key(&column, |(assigned, _)| *assigned)
            .ok()
            .map_or_else(
                || original_values.get(column),
                |position| Some(&self.assignments[position].1),
            )
    }
}

#[derive(Debug)]
enum MutationState {
    Fresh,
    PendingUpdate {
        overlays: Vec<Option<UpdateOverlay>>,
        working_bytes: usize,
    },
    PendingSetNull {
        columns: Vec<usize>,
        working_bytes: usize,
    },
    EncodedUpdate(String),
    Deleted,
}

#[derive(Debug)]
pub(super) struct FrozenRow {
    identity: RowIdentity,
    original_values: Vec<Value>,
    mutation: MutationState,
    update_queued: bool,
}

impl FrozenRow {
    pub(super) fn new(identity: RowIdentity, original_values: Vec<Value>) -> Self {
        Self {
            identity,
            original_values,
            mutation: MutationState::Fresh,
            update_queued: false,
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

    #[cfg(test)]
    pub(super) fn measure_direct_update<'brand>(
        &self,
        update: &PreparedDirectUpdate<'_>,
        encoder: &ValidatedRowEncoder<'_, 'brand>,
    ) -> Result<MeasuredRowEncoding<'brand>> {
        if !matches!(self.mutation, MutationState::Fresh) {
            return Err(direct_conflict(self.identity));
        }
        encoder.measure(self.original_values.len(), |column| {
            update.value_at(&self.original_values, column)
        })
    }

    pub(super) fn install_direct_update(
        &mut self,
        update: &PreparedDirectUpdate<'_>,
        budget: &mut ByteBudget,
    ) -> Result<()> {
        if !matches!(self.mutation, MutationState::Fresh) {
            return Err(direct_conflict(self.identity));
        }

        let (mut overlays, descriptor_bytes) = self.allocate_update_overlays(budget)?;
        let payload_bytes =
            update
                .assignments()
                .iter()
                .try_fold(0_usize, |total, (_, value)| {
                    total
                        .checked_add(value_payload_bytes(value))
                        .ok_or_else(|| budget.limit_error())
                })?;
        if let Err(error) = budget.charge(payload_bytes) {
            drop(overlays);
            budget.release(descriptor_bytes);
            return Err(error);
        }

        for (column, value) in update.assignments() {
            let value = match clone_value(value) {
                Ok(value) => value,
                Err(error) => {
                    drop(overlays);
                    budget.release(payload_bytes);
                    budget.release(descriptor_bytes);
                    return Err(error);
                }
            };
            overlays[*column] = Some(UpdateOverlay { value });
        }
        let working_bytes = descriptor_bytes
            .checked_add(payload_bytes)
            .expect("successful storage-working charges fit in usize");
        self.mutation = MutationState::PendingUpdate {
            overlays,
            working_bytes,
        };
        Ok(())
    }

    pub(super) fn request_update(
        &mut self,
        column: usize,
        value: &Value,
        budget: &mut ByteBudget,
    ) -> Result<bool> {
        if column >= self.original_values.len() {
            return Err(Error::Schema(format!(
                "cascaded UPDATE column {column} is outside a frozen row"
            )));
        }
        if matches!(self.mutation, MutationState::Fresh) {
            let (overlays, working_bytes) = self.allocate_update_overlays(budget)?;
            self.mutation = MutationState::PendingUpdate {
                overlays,
                working_bytes,
            };
        }

        let MutationState::PendingUpdate {
            overlays,
            working_bytes,
        } = &mut self.mutation
        else {
            return Err(direct_conflict(self.identity));
        };
        if let Some(existing) = &overlays[column] {
            if existing.value == *value {
                return Ok(false);
            }
            return Err(update_conflict(self.identity, column));
        }

        let payload_bytes = value_payload_bytes(value);
        budget.charge(payload_bytes)?;
        let value = match clone_value(value) {
            Ok(value) => value,
            Err(error) => {
                budget.release(payload_bytes);
                return Err(error);
            }
        };
        overlays[column] = Some(UpdateOverlay { value });
        *working_bytes = working_bytes
            .checked_add(payload_bytes)
            .ok_or_else(|| budget.limit_error())?;
        Ok(true)
    }

    pub(super) fn effective_value(&self, column: usize) -> Option<&Value> {
        match &self.mutation {
            MutationState::PendingUpdate { overlays, .. } => {
                overlays.get(column).and_then(Option::as_ref).map_or_else(
                    || self.original_values.get(column),
                    |overlay| Some(&overlay.value),
                )
            }
            _ => self.original_values.get(column),
        }
    }

    pub(super) fn mark_update_queued(&mut self, primary_key: usize) -> bool {
        if self.update_queued
            || self.effective_value(primary_key) == self.original_value(primary_key)
        {
            return false;
        }
        self.update_queued = true;
        true
    }

    pub(super) fn clone_effective_value(
        &self,
        column: usize,
        budget: &mut ByteBudget,
    ) -> Result<(Value, usize)> {
        let value = self.effective_value(column).ok_or(Error::Capacity {
            operation: "reading an effective mutation value",
        })?;
        let working_bytes = value_payload_bytes(value);
        budget.charge(working_bytes)?;
        match clone_value(value) {
            Ok(value) => Ok((value, working_bytes)),
            Err(error) => {
                budget.release(working_bytes);
                Err(error)
            }
        }
    }

    fn allocate_update_overlays(
        &self,
        budget: &mut ByteBudget,
    ) -> Result<(Vec<Option<UpdateOverlay>>, usize)> {
        let mut overlays = Vec::new();
        let working_bytes = budget.reserve_exact(
            &mut overlays,
            self.original_values.len(),
            "reserving mutation update overlays",
        )?;
        overlays.resize_with(self.original_values.len(), || None);
        Ok((overlays, working_bytes))
    }

    pub(super) fn request_delete(&mut self, budget: &mut ByteBudget) -> Result<bool> {
        match &self.mutation {
            MutationState::Fresh => {
                self.mutation = MutationState::Deleted;
                Ok(true)
            }
            MutationState::PendingSetNull { working_bytes, .. } => {
                let working_bytes = *working_bytes;
                let previous = std::mem::replace(&mut self.mutation, MutationState::Deleted);
                drop(previous);
                budget.release(working_bytes);
                Ok(true)
            }
            MutationState::Deleted => Ok(false),
            MutationState::PendingUpdate { .. } | MutationState::EncodedUpdate(_) => {
                Err(direct_conflict(self.identity))
            }
        }
    }

    pub(super) fn request_set_null(
        &mut self,
        column: usize,
        budget: &mut ByteBudget,
    ) -> Result<()> {
        match &mut self.mutation {
            MutationState::Fresh => {
                let mut columns = Vec::new();
                let working_bytes = budget.reserve_exact(
                    &mut columns,
                    1,
                    "reserving referential SET NULL columns",
                )?;
                columns.push(column);
                self.mutation = MutationState::PendingSetNull {
                    columns,
                    working_bytes,
                };
                Ok(())
            }
            MutationState::PendingSetNull {
                columns,
                working_bytes,
            } => {
                if columns.len() == columns.capacity() {
                    let charged = budget.reserve_for_push_charged(
                        columns,
                        "reserving referential SET NULL columns",
                    )?;
                    *working_bytes = working_bytes
                        .checked_add(charged)
                        .ok_or_else(|| budget.limit_error())?;
                }
                columns.push(column);
                Ok(())
            }
            MutationState::Deleted => Ok(()),
            MutationState::PendingUpdate { .. } | MutationState::EncodedUpdate(_) => {
                Err(direct_conflict(self.identity))
            }
        }
    }

    pub(super) fn encode_set_null(
        &mut self,
        encoder: &ValidatedRowEncoder<'_, '_>,
        budget: &mut ByteBudget,
    ) -> Result<()> {
        let (columns, working_bytes) = match &mut self.mutation {
            MutationState::PendingSetNull {
                columns,
                working_bytes,
            } => (columns, *working_bytes),
            _ => return Err(direct_conflict(self.identity)),
        };
        columns.sort_unstable();
        columns.dedup();
        let next_column = Cell::new(0);
        let null = Value::Null;
        let measured = encoder.measure(self.original_values.len(), |column| {
            effective_set_null_value(&self.original_values, columns, column, &next_column, &null)
        })?;
        let encoded_len = measured.encoded_len();
        budget.charge(encoded_len)?;
        next_column.set(0);
        let encoded = match encoder.encode(measured, |column| {
            effective_set_null_value(&self.original_values, columns, column, &next_column, &null)
        }) {
            Ok(encoded) => encoded,
            Err(error) => {
                budget.release(encoded_len);
                return Err(error);
            }
        };
        let previous = std::mem::replace(&mut self.mutation, MutationState::EncodedUpdate(encoded));
        drop(previous);
        budget.release(working_bytes);
        Ok(())
    }

    pub(super) fn needs_set_null(&self) -> bool {
        matches!(self.mutation, MutationState::PendingSetNull { .. })
    }

    pub(super) fn measure_effective_update<'brand>(
        &self,
        encoder: &ValidatedRowEncoder<'_, 'brand>,
    ) -> Result<MeasuredRowEncoding<'brand>> {
        let MutationState::PendingUpdate { overlays, .. } = &self.mutation else {
            return Err(direct_conflict(self.identity));
        };
        encoder.measure(self.original_values.len(), |column| {
            effective_update_value(&self.original_values, overlays, column)
        })
    }

    pub(super) fn encode_effective_update<'brand>(
        &mut self,
        encoder: &ValidatedRowEncoder<'_, 'brand>,
        measured: MeasuredRowEncoding<'brand>,
        budget: &mut ByteBudget,
    ) -> Result<()> {
        let (overlays, overlay_working_bytes) = match &self.mutation {
            MutationState::PendingUpdate {
                overlays,
                working_bytes,
            } => (overlays, *working_bytes),
            _ => return Err(direct_conflict(self.identity)),
        };
        let encoded_len = measured.encoded_len();
        budget.charge(encoded_len)?;
        let encoded = match encoder.encode(measured, |column| {
            effective_update_value(&self.original_values, overlays, column)
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

    pub(super) fn needs_update(&self) -> bool {
        matches!(self.mutation, MutationState::PendingUpdate { .. })
    }

    pub(super) fn replacement(&self) -> Result<Option<&str>> {
        match &self.mutation {
            MutationState::EncodedUpdate(encoded) => Ok(Some(encoded)),
            MutationState::Deleted => Ok(None),
            MutationState::Fresh
            | MutationState::PendingUpdate { .. }
            | MutationState::PendingSetNull { .. } => Err(Error::Capacity {
                operation: "reading a planned row replacement",
            }),
        }
    }
}

fn effective_set_null_value<'values>(
    original_values: &'values [Value],
    columns: &[usize],
    column: usize,
    next_column: &Cell<usize>,
    null: &'values Value,
) -> Option<&'values Value> {
    let position = next_column.get();
    if columns.get(position) == Some(&column) {
        next_column.set(position + 1);
        return Some(null);
    }
    original_values.get(column)
}

fn effective_update_value<'values>(
    original_values: &'values [Value],
    overlays: &'values [Option<UpdateOverlay>],
    column: usize,
) -> Option<&'values Value> {
    overlays.get(column).and_then(Option::as_ref).map_or_else(
        || original_values.get(column),
        |overlay| Some(&overlay.value),
    )
}

pub(super) fn decoded_values_bytes(
    column_count: usize,
    encoded_row_bytes: usize,
    budget: &ByteBudget,
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

fn update_conflict(identity: RowIdentity, column: usize) -> Error {
    Error::Constraint(format!(
        "conflicting cascaded updates target column {column} of the row at byte {}",
        identity.start()
    ))
}

fn invalid_identity(offset: usize) -> Error {
    Error::CorruptStorage {
        offset,
        message: String::from("mutation row identity is empty or reversed"),
    }
}
