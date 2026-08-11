//! Budgeted collections for auxiliary storage reconstruction and validation state.
//!
//! The accounting itself lives on [`ByteBudget`]; this module only holds the
//! containers that reserve through one.

use std::cmp::Ordering;

use crate::limits::ByteBudget;
use crate::{Error, Result};

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
        budget: &mut ByteBudget,
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
        budget: &mut ByteBudget,
        operation: &'static str,
    ) -> Result<()> {
        debug_assert!(self.nodes.is_empty());
        if max_items > LINK_MASK {
            return Err(Error::Capacity { operation });
        }
        let _ = budget.reserve_exact(&mut self.nodes, max_items, operation)?;
        Ok(())
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
    use crate::Resource;

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
            let mut budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);
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
    fn packed_avl_reservation_is_fallible_and_capacity_checked() {
        let item_bytes = std::mem::size_of::<WorkingStringNode<'static>>();
        let limit = item_bytes * 5 - 1;
        let mut budget = ByteBudget::new(limit, Resource::StorageWorkingBytes);
        assert!(matches!(
            WorkingStringSet::new(5, &mut budget, "reserving a test index"),
            Err(Error::ResourceLimit {
                resource: Resource::StorageWorkingBytes,
                limit: actual,
            }) if actual == limit
        ));

        let mut budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);
        assert!(matches!(
            WorkingStringSet::new(LINK_MASK + 1, &mut budget, "reserving a test index"),
            Err(Error::Capacity {
                operation: "reserving a test index"
            })
        ));

        let allocation_overflow =
            isize::MAX as usize / std::mem::size_of::<WorkingStringNode<'static>>() + 1;
        let mut budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);
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
