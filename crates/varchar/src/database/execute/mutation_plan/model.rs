//! Frozen row identities, effective overlays, and mutation working memory.

use std::mem::size_of;
use std::ops::Range;

use crate::limits::{check_limit, storage_working_limit};
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

    pub(super) const fn range(self) -> Range<usize> {
        self.start..self.end
    }
}

#[derive(Debug)]
enum MutationState {
    Fresh,
    Deleted,
}

#[derive(Debug)]
pub(super) struct FrozenRow {
    identity: RowIdentity,
    #[cfg_attr(not(test), allow(dead_code))]
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

    pub(super) fn mark_direct_delete(&mut self) -> Result<()> {
        if !matches!(self.mutation, MutationState::Fresh) {
            return Err(direct_conflict(self.identity));
        }
        self.mutation = MutationState::Deleted;
        Ok(())
    }

    pub(super) fn replacement(&self) -> Result<Option<&str>> {
        match &self.mutation {
            MutationState::Deleted => Ok(None),
            MutationState::Fresh => Err(Error::Capacity {
                operation: "reading a planned row replacement",
            }),
        }
    }
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
        if values.len() < values.capacity() {
            return Ok(());
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
        let _ = self.reserve_exact(values, additional, operation)?;
        Ok(())
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
