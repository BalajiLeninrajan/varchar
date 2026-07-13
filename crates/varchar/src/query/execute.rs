//! Execution of compiled scans and bounded row materialization or rewriting.

use fancy_regex::{Error as FancyError, RuntimeError};

use super::{ScanPlan, SelectPlan};
use crate::limits::{Limits, check_limit};
use crate::resolve::ResolvedJoinCondition;
use crate::storage::{self, Candidate, RowRecordRef};
use crate::{ColumnOrigin, Error, Resource, Result, ResultColumn, RowSet, Value};

pub(super) fn select(blob: &str, plan: &SelectPlan<'_>, limits: &Limits) -> Result<RowSet> {
    if plan.sources.len() == 1 {
        select_single_table(blob, plan, limits)
    } else {
        select_join(blob, plan, limits)
    }
}

fn select_single_table(blob: &str, plan: &SelectPlan<'_>, limits: &Limits) -> Result<RowSet> {
    let source = plan
        .sources
        .first()
        .expect("a SELECT plan always has a root source");
    let layout = source.row_layout();
    let mut output_budget =
        ByteBudget::new(limits.max_query_output_bytes(), Resource::QueryOutputBytes);
    output_budget.charge(std::mem::size_of::<RowSet>())?;
    let columns = materialize_result_columns(plan, &mut output_budget)?;
    let working_budget = ByteBudget::new(
        limits.max_query_working_bytes(),
        Resource::QueryWorkingBytes,
    );

    let mut rows = Vec::new();
    let row_structure = row_structure_charge(plan.projection.len(), &output_budget)?;

    for matched in plan.regex.find_iter(blob) {
        let matched = matched.map_err(|error| map_regex_runtime(error, limits))?;
        let row_record = storage::row_record(matched.as_str(), matched.start())?;
        working_budget.check_transient(decoded_row_charge(
            &row_record,
            source.columns.len(),
            &working_budget,
        )?)?;

        let decoded = storage::decode_row(&row_record, layout)?;
        let payload_bytes = plan
            .projection
            .iter()
            .try_fold(0_usize, |total, location| {
                debug_assert_eq!(location.source, 0);
                total
                    .checked_add(value_payload_size(&decoded[location.column]))
                    .ok_or_else(|| output_budget.limit_error())
            })?;
        let row_charge = row_structure
            .checked_add(payload_bytes)
            .ok_or_else(|| output_budget.limit_error())?;
        output_budget.charge(row_charge)?;

        rows.try_reserve(1)
            .map_err(|_| allocation_error("reserving query result rows"))?;
        let mut row = Vec::new();
        row.try_reserve_exact(plan.projection.len())
            .map_err(|_| allocation_error("reserving query result values"))?;
        for location in &plan.projection {
            row.push(clone_result_value(&decoded[location.column])?);
        }
        rows.push(row);
    }

    Ok(RowSet::new(columns, rows))
}

fn select_join(blob: &str, plan: &SelectPlan<'_>, limits: &Limits) -> Result<RowSet> {
    let mut output_budget =
        ByteBudget::new(limits.max_query_output_bytes(), Resource::QueryOutputBytes);
    output_budget.charge(std::mem::size_of::<RowSet>())?;
    let columns = materialize_result_columns(plan, &mut output_budget)?;

    let mut working_budget = ByteBudget::new(
        limits.max_query_working_bytes(),
        Resource::QueryWorkingBytes,
    );
    let source_slots = plan
        .sources
        .len()
        .checked_mul(std::mem::size_of::<Vec<Vec<Value>>>())
        .ok_or_else(|| working_budget.limit_error())?;
    working_budget.charge(source_slots)?;
    let mut source_rows = Vec::new();
    source_rows
        .try_reserve_exact(plan.sources.len())
        .map_err(|_| allocation_error("reserving JOIN source buckets"))?;
    source_rows.resize_with(plan.sources.len(), Vec::new);

    for matched in plan.regex.find_iter(blob) {
        let matched = matched.map_err(|error| map_regex_runtime(error, limits))?;
        let row_record = storage::row_record(matched.as_str(), matched.start())?;
        let table = row_record.table();
        let source_index = plan
            .sources
            .iter()
            .position(|source| source.name == table)
            .ok_or_else(|| Error::RegexRuntime(format!("matched unexpected table {table:?}")))?;
        let source = &plan.sources[source_index];
        let retained_row_charge =
            decoded_row_charge(&row_record, source.columns.len(), &working_budget)?;
        working_budget.charge(retained_row_charge)?;
        let decoded = storage::decode_row(&row_record, source.row_layout())?;
        source_rows[source_index]
            .try_reserve(1)
            .map_err(|_| allocation_error("retaining decoded JOIN rows"))?;
        source_rows[source_index].push(decoded);
    }

    let chosen_slots = plan
        .sources
        .len()
        .checked_mul(std::mem::size_of::<&[Value]>())
        .ok_or_else(|| working_budget.limit_error())?;
    working_budget.charge(chosen_slots)?;
    let mut chosen = Vec::new();
    chosen
        .try_reserve_exact(plan.sources.len())
        .map_err(|_| allocation_error("reserving the chosen JOIN row stack"))?;
    let mut rows = Vec::new();
    let row_structure = row_structure_charge(plan.projection.len(), &output_budget)?;
    let mut output = JoinOutput {
        plan,
        rows: &mut rows,
        output_budget: &mut output_budget,
        join_steps: 0,
        row_structure,
        limits,
    };
    emit_join_rows(0, &mut chosen, &source_rows, &mut output)?;

    Ok(RowSet::new(columns, rows))
}

struct JoinOutput<'a, 'catalog> {
    plan: &'a SelectPlan<'catalog>,
    rows: &'a mut Vec<Vec<Value>>,
    output_budget: &'a mut ByteBudget,
    join_steps: usize,
    row_structure: usize,
    limits: &'a Limits,
}

fn emit_join_rows<'rows>(
    source: usize,
    chosen: &mut Vec<&'rows [Value]>,
    source_rows: &'rows [Vec<Vec<Value>>],
    output: &mut JoinOutput<'_, '_>,
) -> Result<()> {
    if source == source_rows.len() {
        let payload = output
            .plan
            .projection
            .iter()
            .try_fold(0_usize, |total, location| {
                total
                    .checked_add(value_payload_size(
                        &chosen[location.source][location.column],
                    ))
                    .ok_or_else(|| output.output_budget.limit_error())
            })?;
        let charge = output
            .row_structure
            .checked_add(payload)
            .ok_or_else(|| output.output_budget.limit_error())?;
        output.output_budget.charge(charge)?;

        output
            .rows
            .try_reserve(1)
            .map_err(|_| allocation_error("reserving query result rows"))?;
        let mut row = Vec::new();
        row.try_reserve_exact(output.plan.projection.len())
            .map_err(|_| allocation_error("reserving query result values"))?;
        for location in &output.plan.projection {
            row.push(clone_result_value(
                &chosen[location.source][location.column],
            )?);
        }
        output.rows.push(row);
        return Ok(());
    }

    for row in &source_rows[source] {
        chosen.push(row);
        let matches = if source == 0 {
            true
        } else {
            let join = &output.plan.joins[source - 1];
            debug_assert_eq!(join.source, source);
            join_conditions_match(
                chosen,
                &join.conditions,
                &mut output.join_steps,
                output.limits,
            )?
        };
        if matches {
            emit_join_rows(source + 1, chosen, source_rows, output)?;
        }
        chosen.pop();
    }
    Ok(())
}

fn join_conditions_match(
    chosen: &[&[Value]],
    conditions: &[ResolvedJoinCondition],
    join_steps: &mut usize,
    limits: &Limits,
) -> Result<bool> {
    for condition in conditions {
        let left = &chosen[condition.left.source][condition.left.column];
        let right = &chosen[condition.right.source][condition.right.column];
        let comparison_cost = match (left, right) {
            (Value::Text(left), Value::Text(right)) => left
                .len()
                .min(right.len())
                .checked_add(1)
                .ok_or_else(|| join_limit_error(limits))?,
            _ => 1,
        };
        *join_steps = join_steps
            .checked_add(comparison_cost)
            .ok_or_else(|| join_limit_error(limits))?;
        check_limit(*join_steps, limits.max_join_steps(), Resource::JoinSteps)?;
        if matches!(left, Value::Null) || matches!(right, Value::Null) || left != right {
            return Ok(false);
        }
    }
    Ok(true)
}

fn materialize_result_columns(
    plan: &SelectPlan<'_>,
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
        let source = &plan.sources[location.source];
        let column = &source.columns[location.column];
        output_budget.charge(column.name.len())?;
        let label = clone_result_string(&column.name)?;
        output_budget.charge(source.name.len())?;
        let table = clone_result_string(&source.name)?;
        output_budget.charge(column.name.len())?;
        let source_column = clone_result_string(&column.name)?;
        columns.push(ResultColumn::new(
            label,
            ColumnOrigin::new(table, source_column),
            column.data_type,
            column.nullable,
        ));
    }
    Ok(columns)
}

fn row_structure_charge(column_count: usize, budget: &ByteBudget) -> Result<usize> {
    let value_slots = column_count
        .checked_mul(std::mem::size_of::<Value>())
        .ok_or_else(|| budget.limit_error())?;
    // `Vec::try_reserve(1)` may grow the outer row vector geometrically. Keep
    // the existing four-descriptor charge so the logical byte budget remains
    // conservative even when capacity is larger than the current row count.
    let row_descriptors = std::mem::size_of::<Vec<Value>>()
        .checked_mul(4)
        .ok_or_else(|| budget.limit_error())?;
    row_descriptors
        .checked_add(value_slots)
        .ok_or_else(|| budget.limit_error())
}

fn decoded_row_charge(
    row: &RowRecordRef<'_>,
    column_count: usize,
    budget: &ByteBudget,
) -> Result<usize> {
    row_structure_charge(column_count, budget)?
        .checked_add(row.range().len())
        .ok_or_else(|| budget.limit_error())
}

pub(super) fn rewrite_matching_rows<F>(
    candidate: &mut Candidate<'_>,
    plan: &ScanPlan<'_>,
    limits: &Limits,
    mut rewrite: F,
) -> Result<usize>
where
    F: FnMut(Vec<Value>) -> Result<Option<Vec<Value>>>,
{
    let layout = plan.schema.row_layout();
    let mut affected = 0_usize;
    let blob = candidate.source();

    for matched in plan.regex.find_iter(blob) {
        let matched = matched.map_err(|error| map_regex_runtime(error, limits))?;
        let row_record = storage::row_record(matched.as_str(), matched.start())?;
        let values = storage::decode_row(&row_record, layout)?;
        let replacement = rewrite(values)?;
        candidate.rewrite_row(row_record.range(), layout, replacement.as_deref())?;
        affected = affected.checked_add(1).ok_or(Error::Capacity {
            operation: "counting affected rows",
        })?;
    }
    Ok(affected)
}

fn value_payload_size(value: &Value) -> usize {
    match value {
        Value::Text(value) => value.len(),
        Value::Integer(_) | Value::Boolean(_) | Value::Null => 0,
    }
}

fn clone_result_value(value: &Value) -> Result<Value> {
    match value {
        Value::Text(value) => Ok(Value::Text(clone_result_string(value)?)),
        Value::Integer(value) => Ok(Value::Integer(*value)),
        Value::Boolean(value) => Ok(Value::Boolean(*value)),
        Value::Null => Ok(Value::Null),
    }
}

fn clone_result_string(value: &str) -> Result<String> {
    let mut cloned = String::new();
    cloned
        .try_reserve_exact(value.len())
        .map_err(|_| allocation_error("cloning query output text"))?;
    cloned.push_str(value);
    Ok(cloned)
}

struct ByteBudget {
    used: usize,
    limit: usize,
    resource: Resource,
}

impl ByteBudget {
    const fn new(limit: usize, resource: Resource) -> Self {
        Self {
            used: 0,
            limit,
            resource,
        }
    }

    fn charge(&mut self, amount: usize) -> Result<()> {
        let next = self
            .used
            .checked_add(amount)
            .ok_or_else(|| self.limit_error())?;
        check_limit(next, self.limit, self.resource)?;
        self.used = next;
        Ok(())
    }

    fn check_transient(&self, amount: usize) -> Result<()> {
        let peak = self
            .used
            .checked_add(amount)
            .ok_or_else(|| self.limit_error())?;
        check_limit(peak, self.limit, self.resource)
    }

    const fn limit_error(&self) -> Error {
        Error::ResourceLimit {
            resource: self.resource,
            limit: self.limit,
        }
    }
}

const fn allocation_error(operation: &'static str) -> Error {
    Error::Allocation { operation }
}

const fn join_limit_error(limits: &Limits) -> Error {
    Error::ResourceLimit {
        resource: Resource::JoinSteps,
        limit: limits.max_join_steps(),
    }
}

fn map_regex_runtime(error: FancyError, limits: &Limits) -> Error {
    match error {
        FancyError::RuntimeError(RuntimeError::BacktrackLimitExceeded) => Error::ResourceLimit {
            resource: Resource::RegexBacktracking,
            limit: limits.regex_backtrack_limit(),
        },
        other => Error::RegexRuntime(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::ByteBudget;
    use crate::{Error, Resource};

    #[test]
    fn byte_budget_accepts_its_exact_bound_and_rejects_one_more_byte() {
        let mut budget = ByteBudget::new(10, Resource::QueryOutputBytes);

        budget.charge(4).expect("partial charge fits");
        budget.charge(6).expect("exact bound fits");
        assert!(matches!(
            budget.charge(1),
            Err(Error::ResourceLimit {
                resource: Resource::QueryOutputBytes,
                limit: 10,
            })
        ));
        assert_eq!(budget.used, 10, "a rejected charge is not committed");
    }

    #[test]
    fn transient_budget_checks_use_the_same_exact_boundary() {
        let mut budget = ByteBudget::new(10, Resource::QueryWorkingBytes);
        budget.charge(4).expect("retained working bytes fit");

        budget
            .check_transient(6)
            .expect("an exact transient peak fits");
        assert!(matches!(
            budget.check_transient(7),
            Err(Error::ResourceLimit {
                resource: Resource::QueryWorkingBytes,
                limit: 10,
            })
        ));
        assert_eq!(budget.used, 4, "transient checks do not retain a charge");
    }
}
