//! Private accounting for auxiliary storage reconstruction and validation state.

use std::cmp::Ordering;

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

    pub(super) fn clone_text(&mut self, value: &str, operation: &'static str) -> Result<String> {
        self.charge(value.len())?;
        let mut owned = String::new();
        owned
            .try_reserve_exact(value.len())
            .map_err(|_| Error::Allocation { operation })?;
        owned.push_str(value);
        Ok(owned)
    }

    const fn error(&self) -> Error {
        Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit: self.limit,
        }
    }
}

const EMPTY_NODE: usize = usize::MAX;
const BALANCE_SHIFT: u32 = usize::BITS - 2;
const LINK_MASK: usize = usize::MAX >> 2;
const BALANCED_CODE: usize = 1;

/// Fixed-capacity AVL string index whose only allocation is reserved through a working budget.
pub(super) struct WorkingStringSet<'a> {
    nodes: Vec<WorkingStringNode<'a>>,
    root: usize,
}

struct WorkingStringNode<'a> {
    value: &'a str,
    left_link: usize,
    right_link_and_balance: usize,
}

impl<'a> WorkingStringSet<'a> {
    pub(super) fn new(
        max_items: usize,
        budget: &mut WorkingBudget,
        operation: &'static str,
    ) -> Result<Self> {
        let mut set = Self::empty();
        set.reserve(max_items, budget, operation)?;
        Ok(set)
    }

    pub(super) fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            root: EMPTY_NODE,
        }
    }

    pub(super) fn reserve(
        &mut self,
        max_items: usize,
        budget: &mut WorkingBudget,
        operation: &'static str,
    ) -> Result<()> {
        debug_assert!(self.nodes.is_empty());
        if max_items > LINK_MASK {
            return Err(Error::Capacity { operation });
        }
        budget.reserve_exact(&mut self.nodes, max_items, operation)
    }

    pub(super) fn insert(&mut self, value: &'a str) -> bool {
        if self.root == EMPTY_NODE {
            self.root = self.push_node(value);
            return true;
        }
        let (root, inserted, _) = self.insert_at(self.root, value);
        self.root = root;
        inserted
    }

    fn insert_at(&mut self, node: usize, value: &'a str) -> (usize, bool, bool) {
        match insert_values_cmp(value, self.nodes[node].value) {
            Ordering::Equal => (node, false, false),
            Ordering::Less => {
                let left = self.left(node);
                let (left, inserted, grew) = if left == EMPTY_NODE {
                    (self.push_node(value), true, true)
                } else {
                    self.insert_at(left, value)
                };
                if !inserted {
                    return (node, false, false);
                }
                self.set_left(node, left);
                if !grew {
                    return (node, true, false);
                }
                let (root, grew) = self.grow_left(node);
                (root, true, grew)
            }
            Ordering::Greater => {
                let right = self.right(node);
                let (right, inserted, grew) = if right == EMPTY_NODE {
                    (self.push_node(value), true, true)
                } else {
                    self.insert_at(right, value)
                };
                if !inserted {
                    return (node, false, false);
                }
                self.set_right(node, right);
                if !grew {
                    return (node, true, false);
                }
                let (root, grew) = self.grow_right(node);
                (root, true, grew)
            }
        }
    }

    fn push_node(&mut self, value: &'a str) -> usize {
        assert!(
            self.nodes.len() < self.nodes.capacity(),
            "a working string set cannot exceed its reserved item count"
        );
        let index = self.nodes.len();
        self.nodes.push(WorkingStringNode {
            value,
            left_link: 0,
            right_link_and_balance: BALANCED_CODE << BALANCE_SHIFT,
        });
        index
    }

    fn grow_left(&mut self, node: usize) -> (usize, bool) {
        match self.balance(node) {
            1 => {
                self.set_balance(node, 0);
                (node, false)
            }
            0 => {
                self.set_balance(node, -1);
                (node, true)
            }
            -1 => (self.rebalance_left(node), false),
            _ => unreachable!("an AVL balance factor is -1, 0, or 1 before insertion"),
        }
    }

    fn grow_right(&mut self, node: usize) -> (usize, bool) {
        match self.balance(node) {
            -1 => {
                self.set_balance(node, 0);
                (node, false)
            }
            0 => {
                self.set_balance(node, 1);
                (node, true)
            }
            1 => (self.rebalance_right(node), false),
            _ => unreachable!("an AVL balance factor is -1, 0, or 1 before insertion"),
        }
    }

    fn rebalance_left(&mut self, node: usize) -> usize {
        let left = self.left(node);
        match self.balance(left) {
            -1 => {
                let root = self.rotate_right(node);
                self.set_balance(node, 0);
                self.set_balance(root, 0);
                root
            }
            0 => {
                let root = self.rotate_right(node);
                self.set_balance(node, -1);
                self.set_balance(root, 1);
                root
            }
            1 => {
                let pivot = self.right(left);
                let pivot_balance = self.balance(pivot);
                let rotated = self.rotate_left(left);
                self.set_left(node, rotated);
                let root = self.rotate_right(node);
                match pivot_balance {
                    -1 => {
                        self.set_balance(left, 0);
                        self.set_balance(node, 1);
                    }
                    0 => {
                        self.set_balance(left, 0);
                        self.set_balance(node, 0);
                    }
                    1 => {
                        self.set_balance(left, -1);
                        self.set_balance(node, 0);
                    }
                    _ => unreachable!("an AVL balance factor is -1, 0, or 1"),
                }
                self.set_balance(root, 0);
                root
            }
            _ => unreachable!("an AVL balance factor is -1, 0, or 1"),
        }
    }

    fn rebalance_right(&mut self, node: usize) -> usize {
        let right = self.right(node);
        match self.balance(right) {
            1 => {
                let root = self.rotate_left(node);
                self.set_balance(node, 0);
                self.set_balance(root, 0);
                root
            }
            0 => {
                let root = self.rotate_left(node);
                self.set_balance(node, 1);
                self.set_balance(root, -1);
                root
            }
            -1 => {
                let pivot = self.left(right);
                let pivot_balance = self.balance(pivot);
                let rotated = self.rotate_right(right);
                self.set_right(node, rotated);
                let root = self.rotate_left(node);
                match pivot_balance {
                    -1 => {
                        self.set_balance(node, 0);
                        self.set_balance(right, 1);
                    }
                    0 => {
                        self.set_balance(node, 0);
                        self.set_balance(right, 0);
                    }
                    1 => {
                        self.set_balance(node, -1);
                        self.set_balance(right, 0);
                    }
                    _ => unreachable!("an AVL balance factor is -1, 0, or 1"),
                }
                self.set_balance(root, 0);
                root
            }
            _ => unreachable!("an AVL balance factor is -1, 0, or 1"),
        }
    }

    fn rotate_left(&mut self, node: usize) -> usize {
        let pivot = self.right(node);
        debug_assert_ne!(pivot, EMPTY_NODE);
        self.set_right(node, self.left(pivot));
        self.set_left(pivot, node);
        pivot
    }

    fn rotate_right(&mut self, node: usize) -> usize {
        let pivot = self.left(node);
        debug_assert_ne!(pivot, EMPTY_NODE);
        self.set_left(node, self.right(pivot));
        self.set_right(pivot, node);
        pivot
    }

    fn left(&self, node: usize) -> usize {
        decode_link(self.nodes[node].left_link)
    }

    fn set_left(&mut self, node: usize, left: usize) {
        self.nodes[node].left_link = encode_link(left);
    }

    fn right(&self, node: usize) -> usize {
        decode_link(self.nodes[node].right_link_and_balance & LINK_MASK)
    }

    fn set_right(&mut self, node: usize, right: usize) {
        let balance = self.nodes[node].right_link_and_balance & !LINK_MASK;
        self.nodes[node].right_link_and_balance = balance | encode_link(right);
    }

    fn balance(&self, node: usize) -> i8 {
        let code = self.nodes[node].right_link_and_balance >> BALANCE_SHIFT;
        i8::try_from(code).expect("a two-bit AVL balance code fits in i8") - 1
    }

    fn set_balance(&mut self, node: usize, balance: i8) {
        debug_assert!((-1..=1).contains(&balance));
        let code = usize::try_from(balance + 1).expect("an AVL balance code is nonnegative");
        let right = self.nodes[node].right_link_and_balance & LINK_MASK;
        self.nodes[node].right_link_and_balance = (code << BALANCE_SHIFT) | right;
    }
}

fn encode_link(index: usize) -> usize {
    if index == EMPTY_NODE {
        0
    } else {
        index
            .checked_add(1)
            .filter(|link| *link <= LINK_MASK)
            .expect("a working string node index fits its packed link")
    }
}

fn decode_link(link: usize) -> usize {
    link.checked_sub(1).unwrap_or(EMPTY_NODE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_nodes_match_two_borrowed_hash_slots() {
        assert_eq!(
            std::mem::size_of::<WorkingStringNode<'static>>(),
            std::mem::size_of::<Option<&'static str>>() * 2
        );
    }

    #[test]
    fn packed_avl_retains_every_key_in_adversarial_orders() {
        const ITEM_COUNT: usize = 4_096;

        let keys: Vec<_> = (0..ITEM_COUNT)
            .map(|index| format!("column_{index:04}"))
            .collect();
        let mut orders = [
            (0..ITEM_COUNT).collect::<Vec<_>>(),
            (0..ITEM_COUNT).rev().collect(),
            (0..ITEM_COUNT)
                .map(|index| index.wrapping_mul(7_919) % ITEM_COUNT)
                .collect(),
        ];

        for order in &mut orders {
            let mut budget = WorkingBudget::new(usize::MAX);
            let mut set = WorkingStringSet::new(ITEM_COUNT, &mut budget, "reserving a test index")
                .expect("test index reserves");
            reset_working_string_comparisons();
            for &index in order.iter() {
                assert!(set.insert(&keys[index]), "key {index} was already present");
            }
            let (comparisons, _) = working_string_comparisons();
            assert!(
                comparisons <= ITEM_COUNT * 16,
                "{ITEM_COUNT} insertions required {comparisons} comparisons"
            );
            for key in &keys {
                assert!(!set.insert(key), "duplicate key was not retained");
            }
        }
    }

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

    #[test]
    fn packed_avl_reservation_is_fallible_and_capacity_checked() {
        let item_bytes = std::mem::size_of::<WorkingStringNode<'static>>();
        let limit = item_bytes * 5 - 1;
        let mut budget = WorkingBudget::new(limit);
        assert!(matches!(
            WorkingStringSet::new(5, &mut budget, "reserving a test index"),
            Err(Error::ResourceLimit {
                resource: Resource::StorageWorkingBytes,
                limit: actual,
            }) if actual == limit
        ));

        let mut budget = WorkingBudget::new(usize::MAX);
        assert!(matches!(
            WorkingStringSet::new(LINK_MASK + 1, &mut budget, "reserving a test index"),
            Err(Error::Capacity {
                operation: "reserving a test index"
            })
        ));

        let allocation_overflow =
            isize::MAX as usize / std::mem::size_of::<WorkingStringNode<'static>>() + 1;
        let mut budget = WorkingBudget::new(usize::MAX);
        assert!(matches!(
            WorkingStringSet::new(
                allocation_overflow,
                &mut budget,
                "reserving an overflowing test index"
            ),
            Err(Error::Allocation {
                operation: "reserving an overflowing test index"
            })
        ));
    }
}

fn insert_values_cmp(left: &str, right: &str) -> Ordering {
    #[cfg(test)]
    record_working_string_insert_comparison();
    left.cmp(right)
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
