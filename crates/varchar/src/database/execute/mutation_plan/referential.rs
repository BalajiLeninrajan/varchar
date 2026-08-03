//! Indexed referential-action expansion over one immutable source blob.

use std::cell::Cell;
use std::cmp::Ordering;

use super::model::{FrozenRow, PreparedDirectUpdate, RowIdentity, WorkingBudget};
use super::row_record_for_identity;
use crate::storage::{Catalog, ForeignKeyUpdateAction, TableSchema};
use crate::{Error, Result, Value};

const UNFROZEN: usize = usize::MAX;
const RELEVANT_PARENT: u8 = 1 << 0;
const RELEVANT_CHILD: u8 = 1 << 1;

struct ReferentialRelevance {
    tables: Vec<u8>,
    root: usize,
    working_bytes: usize,
}

impl ReferentialRelevance {
    fn is_parent(&self, table: usize) -> bool {
        self.tables[table] & RELEVANT_PARENT != 0
    }

    fn is_child(&self, table: usize) -> bool {
        self.tables[table] & RELEVANT_CHILD != 0
    }

    fn has_children(&self) -> bool {
        self.tables.iter().any(|flags| flags & RELEVANT_CHILD != 0)
    }

    fn release(self, budget: &mut WorkingBudget) {
        let working_bytes = self.working_bytes;
        drop(self);
        budget.release(working_bytes);
    }
}

struct IndexedChildRow<'catalog> {
    identity: RowIdentity,
    schema: &'catalog TableSchema,
    table_order: usize,
    frozen_index: Cell<usize>,
}

struct ReferentialEdge<'catalog, 'blob> {
    referenced_table: &'catalog str,
    parent_key: &'blob str,
    child: usize,
    column: usize,
    on_update: ForeignKeyUpdateAction,
}

pub(super) struct ReferentialIndex<'catalog, 'blob> {
    blob: &'blob str,
    children: Vec<IndexedChildRow<'catalog>>,
    edges: Vec<ReferentialEdge<'catalog, 'blob>>,
    working_bytes: usize,
}

pub(super) fn enforce_update_restrict(
    blob: &str,
    catalog: &Catalog,
    parent_schema: &TableSchema,
    rows: &[FrozenRow],
    update: &PreparedDirectUpdate<'_>,
    budget: &mut WorkingBudget,
) -> Result<()> {
    let Some(primary_key) = parent_schema.primary_key else {
        return Ok(());
    };
    let Some((_, replacement)) = update
        .assignments()
        .iter()
        .find(|(column, _)| *column == primary_key)
    else {
        return Ok(());
    };
    if rows
        .iter()
        .all(|row| row.original_value(primary_key) == Some(replacement))
    {
        return Ok(());
    }

    let index = ReferentialIndex::build(blob, catalog, &parent_schema.name, rows, budget)?;
    index.initialize_direct_rows(rows)?;
    index.enforce_update_restrict(parent_schema, rows, update)?;
    index.release(budget);
    Ok(())
}

impl<'catalog, 'blob> ReferentialIndex<'catalog, 'blob> {
    pub(super) fn build(
        blob: &'blob str,
        catalog: &'catalog Catalog,
        root_table: &str,
        root_rows: &[FrozenRow],
        budget: &mut WorkingBudget,
    ) -> Result<Self> {
        let relevance = referential_relevance(catalog, root_table, budget)?;
        let mut index = Self {
            blob,
            children: Vec::new(),
            edges: Vec::new(),
            working_bytes: 0,
        };
        if !relevance.has_children() {
            relevance.release(budget);
            return Ok(index);
        }

        let (root_keys, root_key_bytes) =
            direct_parent_keys(blob, catalog, root_table, root_rows, budget)?;
        for row in catalog.row_records(blob) {
            let row = row?;
            let (table_order, schema) =
                catalog
                    .table_with_order(row.table())
                    .ok_or_else(|| Error::CorruptStorage {
                        offset: row.range().start,
                        message: String::from("row references an unknown table"),
                    })?;
            if !relevance.is_child(table_order) {
                continue;
            }

            let identity = RowIdentity::new(row.range())?;
            let mut cells = row.cells();
            let mut next_cell = 0;
            let mut child = None;
            for foreign_key in &schema.foreign_keys {
                let skip = foreign_key.column.checked_sub(next_cell).ok_or_else(|| {
                    Error::CorruptStorage {
                        offset: identity.start(),
                        message: String::from(
                            "foreign keys are not in canonical local-column order",
                        ),
                    }
                })?;
                let parent_key = cells.nth(skip).ok_or_else(|| Error::CorruptStorage {
                    offset: identity.start(),
                    message: String::from("foreign-key cell is missing from a validated row"),
                })?;
                next_cell = foreign_key.column.checked_add(1).ok_or(Error::Capacity {
                    operation: "advancing an indexed foreign-key column",
                })?;
                let (referenced_order, _) = catalog
                    .table_with_order(&foreign_key.referenced_table)
                    .ok_or_else(|| Error::CorruptStorage {
                        offset: identity.start(),
                        message: String::from("foreign key references an unknown table"),
                    })?;
                if !relevance.is_parent(referenced_order) || parent_key == "N" {
                    continue;
                }
                if referenced_order == relevance.root
                    && root_keys.binary_search(&parent_key).is_err()
                {
                    continue;
                }

                let child_index = match child {
                    Some(child) => child,
                    None => {
                        let child_index = index.children.len();
                        index.reserve_child(budget)?;
                        index.children.push(IndexedChildRow {
                            identity,
                            schema,
                            table_order,
                            frozen_index: Cell::new(UNFROZEN),
                        });
                        child = Some(child_index);
                        child_index
                    }
                };
                index.reserve_edge(budget)?;
                index.edges.push(ReferentialEdge {
                    referenced_table: &foreign_key.referenced_table,
                    parent_key,
                    child: child_index,
                    column: foreign_key.column,
                    on_update: foreign_key.on_update,
                });
            }
        }

        drop(root_keys);
        budget.release(root_key_bytes);
        relevance.release(budget);
        index.edges.sort_unstable_by(|left, right| {
            edge_key(left, &index.children).cmp(&edge_key(right, &index.children))
        });
        Ok(index)
    }

    pub(super) fn release(self, budget: &mut WorkingBudget) {
        let working_bytes = self.working_bytes;
        drop(self);
        budget.release(working_bytes);
    }

    pub(super) fn initialize_direct_rows(&self, rows: &[FrozenRow]) -> Result<()> {
        for (frozen_index, row) in rows.iter().enumerate() {
            let identity = row.identity();
            let Ok(child) = self
                .children
                .binary_search_by_key(&identity.start(), |child| child.identity.start())
            else {
                continue;
            };
            if self.children[child].identity != identity {
                return Err(Error::CorruptStorage {
                    offset: identity.start(),
                    message: String::from("planned mutation row ranges disagree"),
                });
            }
            self.children[child].frozen_index.set(frozen_index);
        }
        Ok(())
    }

    pub(super) fn enforce_update_restrict(
        &self,
        parent_schema: &TableSchema,
        rows: &[FrozenRow],
        update: &PreparedDirectUpdate<'_>,
    ) -> Result<()> {
        let Some(primary_key) = parent_schema.primary_key else {
            return Ok(());
        };
        let Some((_, replacement)) = update
            .assignments()
            .iter()
            .find(|(column, _)| *column == primary_key)
        else {
            return Ok(());
        };

        for row in rows {
            let old_key = row.original_value(primary_key);
            if old_key == Some(replacement) {
                continue;
            }
            let record = row_record_for_identity(self.blob, row.identity())?;
            let parent_key = record
                .cell(primary_key)
                .ok_or_else(|| Error::CorruptStorage {
                    offset: row.identity().start(),
                    message: String::from("updated row is missing its primary-key cell"),
                })?;
            if let Some(edge) = self
                .matching_edges(&parent_schema.name, parent_key)
                .find(|edge| {
                    edge.on_update == ForeignKeyUpdateAction::Restrict
                        && self.still_references(edge, rows, update, old_key)
                })
            {
                return Err(self.restrict_error(edge));
            }
        }
        Ok(())
    }

    /// Reports whether `edge` still names `old_key` once the statement lands.
    ///
    /// A child row that this same statement rewrites moves with the parent, so
    /// its pre-statement reference is not evidence of a dangling one. Whether
    /// the rewritten reference resolves at all is settled by the candidate-side
    /// foreign-key check, which sees the complete post-statement database.
    fn still_references(
        &self,
        edge: &ReferentialEdge<'_, '_>,
        rows: &[FrozenRow],
        update: &PreparedDirectUpdate<'_>,
        old_key: Option<&Value>,
    ) -> bool {
        let Some(frozen_index) = self.direct_target(edge.child, rows.len()) else {
            return true;
        };
        let child = &rows[frozen_index];
        update
            .assignments()
            .iter()
            .find(|(column, _)| *column == edge.column)
            .map_or_else(
                || child.original_value(edge.column),
                |(_, value)| Some(value),
            )
            .is_none_or(|effective| Some(effective) == old_key)
    }

    /// Maps a referential child back to the direct target that froze it, if the
    /// statement itself named that row.
    ///
    /// Rows induced by referential actions are appended after the direct
    /// prefix, so they are deliberately excluded: their fate depends on the
    /// order the referential queues are drained in.
    fn direct_target(&self, child: usize, direct_rows: usize) -> Option<usize> {
        let frozen_index = self.children[child].frozen_index.get();
        (frozen_index != UNFROZEN && frozen_index < direct_rows).then_some(frozen_index)
    }

    fn matching_edges<'index>(
        &'index self,
        referenced_table: &str,
        parent_key: &str,
    ) -> impl Iterator<Item = &'index ReferentialEdge<'catalog, 'blob>> + 'index {
        let start = self.edges.partition_point(|edge| {
            edge_reference_cmp(edge, referenced_table, parent_key) == Ordering::Less
        });
        let end = start
            + self.edges[start..].partition_point(|edge| {
                edge_reference_cmp(edge, referenced_table, parent_key) == Ordering::Equal
            });
        self.edges[start..end].iter()
    }

    fn restrict_error(&self, edge: &ReferentialEdge<'_, '_>) -> Error {
        let child = &self.children[edge.child];
        Error::Constraint(format!(
            "foreign key {:?}.{:?} restricts mutation of {:?}",
            child.schema.name, child.schema.columns[edge.column].name, edge.referenced_table
        ))
    }

    fn reserve_child(&mut self, budget: &mut WorkingBudget) -> Result<()> {
        let charged = budget.reserve_for_push_charged(
            &mut self.children,
            "reserving indexed referential child rows",
        )?;
        self.add_working_bytes(charged, budget)
    }

    fn reserve_edge(&mut self, budget: &mut WorkingBudget) -> Result<()> {
        let charged = budget
            .reserve_for_push_charged(&mut self.edges, "reserving indexed referential edges")?;
        self.add_working_bytes(charged, budget)
    }

    fn add_working_bytes(&mut self, charged: usize, budget: &WorkingBudget) -> Result<()> {
        self.working_bytes = self
            .working_bytes
            .checked_add(charged)
            .ok_or_else(|| budget.limit_error())?;
        Ok(())
    }
}

fn referential_relevance(
    catalog: &Catalog,
    root_table: &str,
    budget: &mut WorkingBudget,
) -> Result<ReferentialRelevance> {
    let table_count = catalog.table_count();
    let mut tables = Vec::new();
    let working_bytes = budget.reserve_exact(
        &mut tables,
        table_count,
        "reserving relevant referential tables",
    )?;
    tables.resize(table_count, 0);
    let (root, _) = catalog
        .table_with_order(root_table)
        .expect("a compiled mutation names a catalog table");
    tables[root] |= RELEVANT_PARENT;

    let mut has_inbound = false;
    for (_, schema) in catalog.tables() {
        for foreign_key in &schema.foreign_keys {
            if foreign_key.referenced_table != root_table {
                continue;
            }
            has_inbound = true;
        }
    }
    if !has_inbound {
        return Ok(ReferentialRelevance {
            tables,
            root,
            working_bytes,
        });
    }

    for (child, (_, schema)) in catalog.tables().enumerate() {
        if schema.foreign_keys.iter().any(|foreign_key| {
            let (parent, _) = catalog
                .table_with_order(&foreign_key.referenced_table)
                .expect("validated foreign keys reference catalog tables");
            tables[parent] & RELEVANT_PARENT != 0
        }) {
            tables[child] |= RELEVANT_CHILD;
        }
    }

    Ok(ReferentialRelevance {
        tables,
        root,
        working_bytes,
    })
}

fn direct_parent_keys<'blob>(
    blob: &'blob str,
    catalog: &Catalog,
    root_table: &str,
    rows: &[FrozenRow],
    budget: &mut WorkingBudget,
) -> Result<(Vec<&'blob str>, usize)> {
    let schema = catalog
        .table(root_table)
        .expect("a compiled mutation names a catalog table");
    let primary_key = schema.primary_key.ok_or_else(|| Error::CorruptStorage {
        offset: rows[0].identity().start(),
        message: String::from("referenced row table has no primary key"),
    })?;
    let mut keys = Vec::new();
    let working_bytes = budget.reserve_exact(
        &mut keys,
        rows.len(),
        "reserving direct referential parent keys",
    )?;
    for row in rows {
        let record = row_record_for_identity(blob, row.identity())?;
        if record.table() != root_table {
            return Err(Error::CorruptStorage {
                offset: row.identity().start(),
                message: String::from("planned mutation row references the wrong table"),
            });
        }
        let key = record
            .cell(primary_key)
            .ok_or_else(|| Error::CorruptStorage {
                offset: row.identity().start(),
                message: String::from("referenced row is missing its primary-key cell"),
            })?;
        keys.push(key);
    }
    keys.sort_unstable();
    keys.dedup();
    Ok((keys, working_bytes))
}

fn edge_key<'a>(
    edge: &'a ReferentialEdge<'a, 'a>,
    children: &'a [IndexedChildRow<'a>],
) -> (&'a str, &'a str, usize, usize, usize) {
    let child = &children[edge.child];
    (
        edge.referenced_table,
        edge.parent_key,
        child.table_order,
        edge.column,
        child.identity.start(),
    )
}

fn edge_reference_cmp(
    edge: &ReferentialEdge<'_, '_>,
    referenced_table: &str,
    parent_key: &str,
) -> Ordering {
    edge.referenced_table
        .cmp(referenced_table)
        .then_with(|| edge.parent_key.cmp(parent_key))
}
