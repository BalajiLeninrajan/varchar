//! Streaming `SELECT` row collection.

use super::{ByteBudget, allocation_error};
use crate::Result;
use crate::output::{ColumnOrigin, ResultColumn, RowSet};
use crate::query::SelectPlan;
use crate::resolve::ColumnLocation;
use crate::value::Value;

pub(super) struct RowCollector<'plan> {
    projection: &'plan [ColumnLocation],
    output_budget: ByteBudget,
    row_structure: usize,
    rows: Vec<Vec<Value>>,
}

impl<'plan> RowCollector<'plan> {
    pub(super) fn new(plan: &'plan SelectPlan<'_, '_>, output_budget: ByteBudget) -> Result<Self> {
        let row_structure = row_structure_charge(plan.projection.len(), &output_budget)?;
        Ok(Self {
            projection: &plan.projection,
            output_budget,
            row_structure,
            rows: Vec::new(),
        })
    }

    pub(super) fn collect(&mut self, sources: &[&[Value]]) -> Result<()> {
        collect_streaming(
            &mut self.rows,
            self.projection,
            sources,
            self.row_structure,
            &mut self.output_budget,
        )
    }

    pub(super) fn finish(self, columns: Vec<ResultColumn>) -> Result<RowSet> {
        Ok(RowSet::new(columns, self.rows))
    }
}

pub(super) fn materialize_result_columns(
    plan: &SelectPlan<'_, '_>,
    output_budget: &mut ByteBudget,
) -> Result<Vec<ResultColumn>> {
    let column_slots = plan
        .projection
        .len()
        .checked_mul(std::mem::size_of::<ResultColumn>())
        .ok_or_else(|| output_budget.limit_error())?;
    output_budget.charge(column_slots)?;

    let mut columns = Vec::new();
    columns
        .try_reserve_exact(plan.projection.len())
        .map_err(|_| allocation_error("reserving query result columns"))?;
    for location in &plan.projection {
        let source = plan.sources[location.source];
        let column = &source.columns[location.column];
        output_budget.charge(column.name.len())?;
        let label = clone_string(&column.name, "cloning query output text")?;
        output_budget.charge(source.name.len())?;
        let table = clone_string(&source.name, "cloning query output text")?;
        output_budget.charge(column.name.len())?;
        let source_column = clone_string(&column.name, "cloning query output text")?;
        columns.push(ResultColumn::new(
            label,
            ColumnOrigin::new(table, source_column),
            column.data_type,
            column.nullable,
        ));
    }
    Ok(columns)
}

fn collect_streaming(
    rows: &mut Vec<Vec<Value>>,
    projection: &[ColumnLocation],
    sources: &[&[Value]],
    row_structure: usize,
    output_budget: &mut ByteBudget,
) -> Result<()> {
    let payload_bytes = projected_payload_size(projection, sources, output_budget)?;
    let row_charge = row_structure
        .checked_add(payload_bytes)
        .ok_or_else(|| output_budget.limit_error())?;
    output_budget.charge(row_charge)?;

    rows.try_reserve(1)
        .map_err(|_| allocation_error("reserving query result rows"))?;
    let mut row = Vec::new();
    row.try_reserve_exact(projection.len())
        .map_err(|_| allocation_error("reserving query result values"))?;
    for location in projection {
        row.push(clone_value(
            value_at(sources, *location),
            "cloning query output text",
        )?);
    }
    rows.push(row);
    Ok(())
}

fn row_structure_charge(column_count: usize, budget: &ByteBudget) -> Result<usize> {
    let value_slots = column_count
        .checked_mul(std::mem::size_of::<Value>())
        .ok_or_else(|| budget.limit_error())?;
    // `Vec::try_reserve(1)` may grow the outer row vector geometrically. Keep
    // four row descriptors in the logical charge to account conservatively.
    let row_descriptors = std::mem::size_of::<Vec<Value>>()
        .checked_mul(4)
        .ok_or_else(|| budget.limit_error())?;
    row_descriptors
        .checked_add(value_slots)
        .ok_or_else(|| budget.limit_error())
}

fn projected_payload_size(
    projection: &[ColumnLocation],
    sources: &[&[Value]],
    budget: &ByteBudget,
) -> Result<usize> {
    payload_size(
        projection
            .iter()
            .map(|location| value_at(sources, *location)),
        budget,
    )
}

fn payload_size<'value>(
    values: impl IntoIterator<Item = &'value Value>,
    budget: &ByteBudget,
) -> Result<usize> {
    values.into_iter().try_fold(0_usize, |total, value| {
        total
            .checked_add(value_payload_size(value))
            .ok_or_else(|| budget.limit_error())
    })
}

fn value_at<'value>(sources: &[&'value [Value]], location: ColumnLocation) -> &'value Value {
    &sources[location.source][location.column]
}

fn value_payload_size(value: &Value) -> usize {
    match value {
        Value::Text(value) => value.len(),
        Value::Integer(_) | Value::Boolean(_) | Value::Null => 0,
    }
}

fn clone_value(value: &Value, operation: &'static str) -> Result<Value> {
    match value {
        Value::Text(value) => Ok(Value::Text(clone_string(value, operation)?)),
        Value::Integer(value) => Ok(Value::Integer(*value)),
        Value::Boolean(value) => Ok(Value::Boolean(*value)),
        Value::Null => Ok(Value::Null),
    }
}

fn clone_string(value: &str, operation: &'static str) -> Result<String> {
    let mut cloned = String::new();
    cloned
        .try_reserve_exact(value.len())
        .map_err(|_| allocation_error(operation))?;
    cloned.push_str(value);
    Ok(cloned)
}
