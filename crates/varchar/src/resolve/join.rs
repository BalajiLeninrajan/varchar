//! Validation and positional resolution of inner-join conditions.

use super::column::{ColumnLocation, resolve_column};
use crate::sql::Select;
use crate::storage::TableSchema;
use crate::{Error, Result};

pub(crate) struct ResolvedJoinCondition {
    pub(crate) left: ColumnLocation,
    pub(crate) right: ColumnLocation,
}

pub(crate) struct ResolvedJoin {
    pub(crate) source: usize,
    pub(crate) conditions: Vec<ResolvedJoinCondition>,
}

pub(super) fn resolve_joins(
    statement: &Select,
    schemas: &[&TableSchema],
) -> Result<Vec<ResolvedJoin>> {
    let mut joins = Vec::with_capacity(statement.joins.len());
    for (join_index, join) in statement.joins.iter().enumerate() {
        let source = join_index + 1;
        let visible = &schemas[..=source];
        let mut conditions = Vec::with_capacity(join.conditions.len());
        let mut connects_source = false;
        for condition in &join.conditions {
            let left = resolve_column(visible, &condition.left)?;
            let right = resolve_column(visible, &condition.right)?;
            connects_source |= (left.source == source && right.source < source)
                || (right.source == source && left.source < source);
            let left_type = schemas[left.source].columns[left.column].data_type;
            let right_type = schemas[right.source].columns[right.column].data_type;
            if left_type != right_type {
                return Err(Error::type_error(format!(
                    "JOIN columns {:?}.{:?} and {:?}.{:?} have different types",
                    schemas[left.source].name,
                    schemas[left.source].columns[left.column].name,
                    schemas[right.source].name,
                    schemas[right.source].columns[right.column].name
                )));
            }
            conditions.push(ResolvedJoinCondition { left, right });
        }
        if !connects_source {
            return Err(Error::schema(format!(
                "JOIN for table {:?} must connect it to an earlier table",
                join.table
            )));
        }
        joins.push(ResolvedJoin { source, conditions });
    }
    Ok(joins)
}
