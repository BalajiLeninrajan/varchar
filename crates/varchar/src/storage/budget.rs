//! Private accounting for auxiliary storage reconstruction and validation state.

use crate::{Error, Resource, Result};

/// The smallest reservation a geometrically grown working vector charges for.
const MIN_GROWTH_ITEMS: usize = 2;

/// The reservation growth moves to once `reserved` items have been spent.
///
/// Capacity grows by half because the derived working limit fixes the affordable slack. The
/// densest keyed blob a database can hold spends eight bytes on a row (`~R|t|Ta;`) whose key
/// costs `size_of::<&str>()` bytes to index, so an exactly sized index already spends half of
/// the four-times-database-size working limit and growth may only claim the other half.
/// Growing by half stays inside that headroom; doubling would consume all of it and reject
/// dense blobs that the sizing pass this growth replaced used to admit.
fn grown_items(reserved: usize) -> usize {
    reserved
        .saturating_add(reserved / 2)
        .max(reserved.saturating_add(1))
        .max(MIN_GROWTH_ITEMS)
}

/// The reservation a geometrically grown vector has been charged for at `len` items.
///
/// Growth follows `grown_items` from zero, so what a vector has been charged is a property of
/// how many times it was appended to. It is deliberately not `Vec::capacity`, which
/// `try_reserve_exact` explicitly allows an allocator to round up: charging from a rounded-up
/// capacity would make the working limit allocator-dependent, and releasing one would hand the
/// budget back bytes it was never charged.
fn charged_growth_items(len: usize) -> usize {
    let mut reserved = 0;
    while reserved < len {
        reserved = grown_items(reserved);
    }
    reserved
}

/// The storage-working limit is intentionally derived rather than public API.
pub(super) const fn working_limit(max_database_bytes: usize) -> usize {
    max_database_bytes.saturating_mul(4)
}

#[derive(Debug)]
pub(super) struct WorkingBudget {
    limit: usize,
    used: usize,
}

impl WorkingBudget {
    pub(super) const fn new(limit: usize) -> Self {
        Self { limit, used: 0 }
    }

    pub(super) fn charge(&mut self, bytes: usize) -> Result<()> {
        let used = self.used.checked_add(bytes).ok_or_else(|| self.error())?;
        if used > self.limit {
            return Err(self.error());
        }
        self.used = used;
        Ok(())
    }

    pub(super) fn charge_items<T>(&mut self, count: usize) -> Result<()> {
        let bytes = count
            .checked_mul(std::mem::size_of::<T>())
            .ok_or_else(|| self.error())?;
        self.charge(bytes)
    }

    pub(super) fn reserve_exact<T>(
        &mut self,
        values: &mut Vec<T>,
        additional: usize,
        operation: &'static str,
    ) -> Result<()> {
        self.charge_items::<T>(additional)?;
        values
            .try_reserve_exact(additional)
            .map_err(|_| Error::Allocation { operation })
    }

    /// Grows `values` geometrically so a fill pass never needs a preceding sizing pass.
    ///
    /// Returns the bytes charged. That count, and never anything read back off the grown
    /// vector, is the ledger a caller accumulates: the charge is taken against the reservation
    /// `charged_growth_items` derives from the appends themselves, so an allocator that rounds
    /// a `try_reserve_exact` request up changes neither what was charged nor what is owed back.
    pub(super) fn reserve_growth<T>(
        &mut self,
        values: &mut Vec<T>,
        operation: &'static str,
    ) -> Result<usize> {
        let reserved = charged_growth_items(values.len());
        let grown = grown_items(reserved);
        let added = grown - reserved;
        self.charge_items::<T>(added)?;
        values
            .try_reserve_exact(grown - values.len())
            .map_err(|_| Error::Allocation { operation })?;
        Ok(added * std::mem::size_of::<T>())
    }

    /// Appends to a budgeted vector, charging and growing only when its reservation is spent.
    ///
    /// Returns the bytes this append charged, which is zero whenever the reservation already
    /// charged for had room left.
    pub(super) fn push_charged<T>(
        &mut self,
        values: &mut Vec<T>,
        value: T,
        operation: &'static str,
    ) -> Result<usize> {
        let added = if values.len() == charged_growth_items(values.len()) {
            self.reserve_growth(values, operation)?
        } else {
            0
        };
        values.push(value);
        Ok(added)
    }

    const fn error(&self) -> Error {
        Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit: self.limit,
        }
    }
}

#[cfg(test)]
std::thread_local! {
    static WORKING_STRING_COMPARISONS: std::cell::Cell<(usize, usize)> = const {
        std::cell::Cell::new((0, 0))
    };
}

#[cfg(test)]
pub(super) fn record_working_string_insert_comparison() {
    WORKING_STRING_COMPARISONS.with(|comparisons| {
        let (insert, lookup) = comparisons.get();
        comparisons.set((insert + 1, lookup));
    });
}

#[cfg(test)]
pub(super) fn record_working_string_lookup_comparison() {
    WORKING_STRING_COMPARISONS.with(|comparisons| {
        let (insert, lookup) = comparisons.get();
        comparisons.set((insert, lookup + 1));
    });
}

#[cfg(test)]
pub(super) fn reset_working_string_comparisons() {
    WORKING_STRING_COMPARISONS.with(|comparisons| comparisons.set((0, 0)));
}

#[cfg(test)]
pub(super) fn working_string_comparisons() -> (usize, usize) {
    WORKING_STRING_COMPARISONS.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometric_growth_charges_exactly_what_its_appends_report() {
        const ITEM_COUNT: usize = 1_000;

        let item_bytes = std::mem::size_of::<usize>();
        let mut budget = WorkingBudget::new(usize::MAX);
        let mut values = Vec::new();
        let mut charged = 0;
        for value in 0..ITEM_COUNT {
            charged += budget
                .push_charged(&mut values, value, "growing a test vector")
                .expect("an unlimited budget always grows");
        }

        assert_eq!(values, (0..ITEM_COUNT).collect::<Vec<_>>());
        // What the appends reported is the whole charge, and therefore the whole of what a
        // release owes back. `Vec::capacity` is deliberately not the yardstick: an allocator
        // may round a `try_reserve_exact` request up, and a release measured off a rounded-up
        // capacity would hand back bytes the budget was never charged.
        assert_eq!(budget.used, charged);
        assert_eq!(charged, charged_growth_items(ITEM_COUNT) * item_bytes);
        // 0, 2, 3, 4, 6, 9, ... 1066: growing by half reserves 1066 items for 1000 appends,
        // so this pins the growth factor as well as the ledger.
        assert_eq!(charged, 1_066 * item_bytes);
        assert!(
            values.capacity() >= 1_066,
            "the allocator may hold more than the reservation charged for, never less"
        );
    }

    #[test]
    fn geometric_growth_fails_with_the_working_bytes_resource() {
        let item_bytes = std::mem::size_of::<usize>();
        let limit = item_bytes * 3;
        let mut budget = WorkingBudget::new(limit);
        let mut values: Vec<usize> = Vec::new();
        let mut charged = 0;

        for value in 0..3 {
            charged += budget
                .push_charged(&mut values, value, "growing a test vector")
                .expect("the first three items fit the limit");
        }
        assert_eq!(charged, item_bytes * 3);
        assert!(matches!(
            budget.push_charged(&mut values, 3, "growing a test vector"),
            Err(Error::ResourceLimit {
                resource: Resource::StorageWorkingBytes,
                limit: actual,
            }) if actual == limit
        ));
        assert_eq!(
            budget.used, charged,
            "a refused growth leaves the budget agreeing with what the appends reported"
        );
    }
}
