//! Resource policy shared by request validation, resolution, and execution.

use std::fmt;

use crate::{Error, Result};

const MIB: usize = 1024 * 1024;

/// A configurable resource governed by [`Limits`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Resource {
    /// The authoritative database string.
    DatabaseBytes,
    /// Live auxiliary state used for storage reconstruction, validation, and mutation planning.
    StorageWorkingBytes,
    /// One SQL statement.
    SqlBytes,
    /// Predicate units in one `WHERE` expression.
    WherePredicates,
    /// Predicate units across all `CHECK` declarations on one table.
    CheckPredicates,
    /// Source tables participating in one `SELECT`.
    JoinSources,
    /// One generated regular expression.
    GeneratedRegexBytes,
    /// Conservatively accounted transient and retained working state for `SELECT`.
    QueryWorkingBytes,
    /// Returned query data, explanations, and resolved-projection planning.
    QueryOutputBytes,
    /// Value-comparison work performed by a join.
    JoinSteps,
    /// Backtracking performed by one regex search, and wildcard backtracking
    /// performed by the `LIKE` matcher across one statement.
    RegexBacktracking,
}

impl fmt::Display for Resource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DatabaseBytes => "database bytes",
            Self::StorageWorkingBytes => "storage working bytes",
            Self::SqlBytes => "SQL bytes",
            Self::WherePredicates => "WHERE predicates",
            Self::CheckPredicates => "CHECK predicates",
            Self::JoinSources => "JOIN sources",
            Self::GeneratedRegexBytes => "generated regex bytes",
            Self::QueryWorkingBytes => "query working bytes",
            Self::QueryOutputBytes => "query output bytes",
            Self::JoinSteps => "JOIN execution steps",
            Self::RegexBacktracking => "regex backtracking steps",
        })
    }
}

/// Resource bounds applied by the platform-neutral database core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Maximum size of the authoritative database string, in UTF-8 bytes.
    pub max_database_bytes: usize,
    /// Maximum size of one SQL statement, in UTF-8 bytes.
    pub max_sql_bytes: usize,
    /// Maximum predicate units in one `WHERE` expression or cumulatively across
    /// all `CHECK` declarations on one table.
    ///
    /// Ordinary predicate leaves consume one unit, while `IN` consumes one
    /// unit per list member. Logical operators and parentheses consume none.
    pub max_predicates: usize,
    /// Maximum number of source tables participating in one `SELECT`.
    pub max_join_sources: usize,
    /// Maximum size of one generated regular expression, in UTF-8 bytes.
    ///
    /// The regex compiler also receives this value as an approximate compiled
    /// delegate safeguard. Refusals from that safeguard remain
    /// [`crate::Error::RegexCompile`] errors; [`Resource::GeneratedRegexBytes`]
    /// describes generated pattern text, not compiled engine state.
    pub max_pattern_bytes: usize,
    /// Maximum logical `SELECT` working-state charge, in conservatively
    /// accounted bytes.
    ///
    /// An unordered single-table query retains one decoded row at a time, so its
    /// decoded-row charge is a peak-per-row budget rather than a cumulative scan
    /// budget. An ordered query additionally retains a projection, one owned value
    /// per sort key (including text payload), pending-row descriptors, and a
    /// tie-breaking ordinal per pending row. With a `LIMIT` it retains at most
    /// `OFFSET + LIMIT` of those rows and refunds the charge for every row it
    /// evicts, so the charge tracks the pagination window rather than the number
    /// of qualifying rows; without one it retains every qualifying row, and
    /// `LIMIT 0` instead skips execution working state entirely. Joined queries
    /// also charge each decoded row transiently, then cumulatively charge rows
    /// retained after source-local residuals, the chosen-row pointer stack, and
    /// one reusable residual-evaluation stack. This does not govern `UPDATE` or
    /// `DELETE`; returned rows have a separate limit. Charges include
    /// target-layout sizes, so an exact boundary can differ between 32-bit and
    /// 64-bit builds.
    pub max_query_working_bytes: usize,
    /// Maximum conservatively accounted bytes for one materialized `SELECT`
    /// result after pagination or one `SELECT` explanation.
    ///
    /// The resolver also uses this value as an independent bound on the
    /// expanded projection-location plan before query compilation.
    ///
    /// Charges include target-layout sizes, so an exact boundary can differ
    /// between 32-bit and 64-bit builds.
    pub max_query_output_bytes: usize,
    /// Maximum amount of value-comparison work performed while joining rows.
    pub max_join_steps: usize,
    /// Per-search work limit for generated regexes, and the wildcard
    /// backtracking budget one statement shares across every `LIKE` it
    /// evaluates outside a scan pattern, in `WHERE` and in `CHECK` alike.
    ///
    /// A matched `LIKE` runs segment by segment and scans forward without
    /// charge, so ordinary matching is bounded by the text rather than by this
    /// limit. Only re-comparison beyond a forward scan is charged, and it is
    /// charged once for the statement rather than once per row or predicate.
    pub regex_backtrack_limit: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_database_bytes: 64 * MIB,
            max_sql_bytes: 64 * 1024,
            max_predicates: 64,
            max_join_sources: 64,
            max_pattern_bytes: 8 * MIB,
            max_query_working_bytes: 32 * MIB,
            max_query_output_bytes: 32 * MIB,
            max_join_steps: 1_000_000,
            regex_backtrack_limit: 1_000_000,
        }
    }
}

pub(crate) const fn storage_working_limit(max_database_bytes: usize) -> usize {
    max_database_bytes.saturating_mul(4)
}

pub(crate) fn check_limit(actual: usize, limit: usize, resource: Resource) -> Result<()> {
    if actual > limit {
        Err(Error::ResourceLimit { resource, limit })
    } else {
        Ok(())
    }
}

/// The smallest reservation a geometrically grown budgeted vector charges for.
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
pub(crate) fn charged_growth_items(len: usize) -> usize {
    let mut reserved = 0;
    while reserved < len {
        reserved = grown_items(reserved);
    }
    reserved
}

/// Live byte accounting for one [`Resource`].
///
/// Every budgeted subsystem shares this type: the resource it reports is a
/// field rather than a property of the module that declared it, so query
/// output, query working state, and storage working state all charge, release,
/// and grow through the same arithmetic.
#[derive(Debug)]
pub(crate) struct ByteBudget {
    pub(crate) used: usize,
    limit: usize,
    resource: Resource,
}

impl ByteBudget {
    pub(crate) const fn new(limit: usize, resource: Resource) -> Self {
        Self {
            used: 0,
            limit,
            resource,
        }
    }

    pub(crate) fn charge(&mut self, amount: usize) -> Result<()> {
        let next = self
            .used
            .checked_add(amount)
            .ok_or_else(|| self.limit_error())?;
        check_limit(next, self.limit, self.resource)?;
        self.used = next;
        Ok(())
    }

    /// Returns bytes to the budget when a charged allocation is dropped again.
    ///
    /// Reservations refund themselves when the allocation fails, and decoded
    /// values are released once validation has read them, so the budget has to
    /// track live bytes rather than a running total. Every release mirrors a
    /// charge this budget accepted, which the debug assertion pins down; the
    /// saturating subtraction then keeps an accounting slip from either
    /// panicking or wrapping in release builds.
    pub(crate) fn release(&mut self, amount: usize) {
        debug_assert!(
            amount <= self.used,
            "only a live charge can be released to a byte budget"
        );
        self.used = self.used.saturating_sub(amount);
    }

    /// Charges the bytes `count` values of `T` occupy.
    pub(crate) fn charge_items<T>(&mut self, count: usize) -> Result<usize> {
        let bytes = count
            .checked_mul(size_of::<T>())
            .ok_or_else(|| self.limit_error())?;
        self.charge(bytes)?;
        Ok(bytes)
    }

    /// Charges and reserves exactly `additional` more slots, returning the bytes charged.
    ///
    /// A failed reservation refunds its charge, so a budget outlives the
    /// allocation failures a caller recovers from.
    pub(crate) fn reserve_exact<T>(
        &mut self,
        values: &mut Vec<T>,
        additional: usize,
        operation: &'static str,
    ) -> Result<usize> {
        let bytes = self.charge_items::<T>(additional)?;
        if values.try_reserve_exact(additional).is_err() {
            self.release(bytes);
            return Err(Error::Allocation { operation });
        }
        Ok(bytes)
    }

    /// Grows `values` by half so a fill pass never needs a preceding sizing pass.
    ///
    /// Returns the bytes charged, as every other reserving helper here does. That count, and
    /// never anything read back off the grown vector, is the ledger a caller accumulates: the
    /// charge is taken against the reservation [`charged_growth_items`] derives from the
    /// appends themselves, so an allocator that rounds a `try_reserve_exact` request up
    /// changes neither what was charged nor what [`Self::release`] is owed.
    pub(crate) fn reserve_growth<T>(
        &mut self,
        values: &mut Vec<T>,
        operation: &'static str,
    ) -> Result<usize> {
        let reserved = charged_growth_items(values.len());
        let grown = grown_items(reserved);
        let bytes = self.charge_items::<T>(grown - reserved)?;
        if values.try_reserve_exact(grown - values.len()).is_err() {
            self.release(bytes);
            return Err(Error::Allocation { operation });
        }
        Ok(bytes)
    }

    /// Appends to a budgeted vector, charging and growing only when its reservation is spent.
    ///
    /// Returns the bytes this append charged, which is zero whenever the reservation already
    /// charged for had room left.
    pub(crate) fn push_charged<T>(
        &mut self,
        values: &mut Vec<T>,
        value: T,
        operation: &'static str,
    ) -> Result<usize> {
        let bytes = if values.len() == charged_growth_items(values.len()) {
            self.reserve_growth(values, operation)?
        } else {
            0
        };
        values.push(value);
        Ok(bytes)
    }

    /// Charges and copies `value` into an owned string.
    pub(crate) fn clone_text(&mut self, value: &str, operation: &'static str) -> Result<String> {
        self.charge(value.len())?;
        let mut owned = String::new();
        owned
            .try_reserve_exact(value.len())
            .map_err(|_| Error::Allocation { operation })?;
        owned.push_str(value);
        Ok(owned)
    }

    pub(crate) const fn limit_error(&self) -> Error {
        Error::ResourceLimit {
            resource: self.resource,
            limit: self.limit,
        }
    }
}

#[cfg(test)]
mod tests;
