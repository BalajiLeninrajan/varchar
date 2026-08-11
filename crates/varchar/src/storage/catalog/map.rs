//! Fallibly allocated ordered maps used by the reconstructed catalog.

use super::super::budget::WorkingBudget;
use crate::{Error, Result};

const EMPTY_NODE: usize = usize::MAX;

fn next_capacity(capacity: usize) -> Option<usize> {
    if capacity == 0 {
        Some(1)
    } else {
        capacity.checked_mul(2)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::storage) struct CatalogMap<V> {
    nodes: Vec<MapNode<V>>,
    root: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MapNode<V> {
    key: String,
    value: V,
    left: usize,
    right: usize,
    height: u8,
}

impl<V> CatalogMap<V> {
    pub(in crate::storage) const fn new() -> Self {
        Self {
            nodes: Vec::new(),
            root: EMPTY_NODE,
        }
    }

    pub(in crate::storage) fn get(&self, key: &str) -> Option<&V> {
        self.find(key).map(|index| &self.nodes[index].value)
    }

    pub(in crate::storage) fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        let index = self.find(key)?;
        Some(&mut self.nodes[index].value)
    }

    pub(in crate::storage) fn contains_key(&self, key: &str) -> bool {
        self.find(key).is_some()
    }

    pub(in crate::storage) fn values(&self) -> impl Iterator<Item = &V> {
        self.nodes.iter().map(|node| &node.value)
    }

    pub(in crate::storage) fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.nodes
            .iter()
            .map(|node| (node.key.as_str(), &node.value))
    }

    pub(in crate::storage) fn insert_new(
        &mut self,
        key: String,
        value: V,
        budget: &mut WorkingBudget,
        operation: &'static str,
    ) -> Result<()> {
        debug_assert!(!self.contains_key(&key));
        // Callers charge owned keys and values; this is the AVL node's logical index state.
        budget.charge_items::<usize>(3)?;
        if self.nodes.len() == self.nodes.capacity() {
            let target =
                next_capacity(self.nodes.capacity()).ok_or(Error::Capacity { operation })?;
            self.nodes
                .try_reserve_exact(target - self.nodes.capacity())
                .map_err(|_| Error::Allocation { operation })?;
        }

        let index = self.nodes.len();
        self.nodes.push(MapNode {
            key,
            value,
            left: EMPTY_NODE,
            right: EMPTY_NODE,
            height: 1,
        });
        if self.root == EMPTY_NODE {
            self.root = index;
        } else {
            self.root = self.insert_at(self.root, index);
        }
        Ok(())
    }

    fn find(&self, key: &str) -> Option<usize> {
        let mut node = self.root;
        while node != EMPTY_NODE {
            match key.cmp(&self.nodes[node].key) {
                std::cmp::Ordering::Less => node = self.nodes[node].left,
                std::cmp::Ordering::Equal => return Some(node),
                std::cmp::Ordering::Greater => node = self.nodes[node].right,
            }
        }
        None
    }

    fn insert_at(&mut self, node: usize, inserted: usize) -> usize {
        match self.nodes[inserted].key.cmp(&self.nodes[node].key) {
            std::cmp::Ordering::Less => {
                let left = self.nodes[node].left;
                self.nodes[node].left = if left == EMPTY_NODE {
                    inserted
                } else {
                    self.insert_at(left, inserted)
                };
            }
            std::cmp::Ordering::Greater => {
                let right = self.nodes[node].right;
                self.nodes[node].right = if right == EMPTY_NODE {
                    inserted
                } else {
                    self.insert_at(right, inserted)
                };
            }
            std::cmp::Ordering::Equal => {
                unreachable!("catalog map insertion requires a new key")
            }
        }
        self.rebalance(node)
    }

    fn rebalance(&mut self, node: usize) -> usize {
        self.update_height(node);
        let balance = self.balance(node);
        if balance > 1 {
            let left = self.nodes[node].left;
            if self.balance(left) < 0 {
                self.nodes[node].left = self.rotate_left(left);
            }
            self.rotate_right(node)
        } else if balance < -1 {
            let right = self.nodes[node].right;
            if self.balance(right) > 0 {
                self.nodes[node].right = self.rotate_right(right);
            }
            self.rotate_left(node)
        } else {
            node
        }
    }

    fn rotate_left(&mut self, node: usize) -> usize {
        let pivot = self.nodes[node].right;
        debug_assert_ne!(pivot, EMPTY_NODE);
        self.nodes[node].right = self.nodes[pivot].left;
        self.nodes[pivot].left = node;
        self.update_height(node);
        self.update_height(pivot);
        pivot
    }

    fn rotate_right(&mut self, node: usize) -> usize {
        let pivot = self.nodes[node].left;
        debug_assert_ne!(pivot, EMPTY_NODE);
        self.nodes[node].left = self.nodes[pivot].right;
        self.nodes[pivot].right = node;
        self.update_height(node);
        self.update_height(pivot);
        pivot
    }

    fn update_height(&mut self, node: usize) {
        let child_height = self
            .height(self.nodes[node].left)
            .max(self.height(self.nodes[node].right));
        self.nodes[node].height = child_height
            .checked_add(1)
            .expect("an AVL tree height fits in u8 for every usize-sized allocation");
    }

    fn balance(&self, node: usize) -> i16 {
        i16::from(self.height(self.nodes[node].left))
            - i16::from(self.height(self.nodes[node].right))
    }

    fn height(&self, node: usize) -> u8 {
        if node == EMPTY_NODE {
            0
        } else {
            self.nodes[node].height
        }
    }
}

#[cfg(test)]
mod tests;
