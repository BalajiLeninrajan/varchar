//! Execution of compiled scans and bounded row materialization or rewriting.

use fancy_regex::{Error as FancyError, RuntimeError};

use super::{ScanPlan, SelectPlan};
use crate::limits::{Limits, check_limit};
use crate::resolve::ResolvedJoinCondition;
use crate::storage::{self, RowLayout};
use crate::{Column, Error, Result, RowSet, Value};

pub(super) fn select(blob: &str, plan: &SelectPlan, limits: &Limits) -> Result<RowSet> {
    if plan.sources.len() == 1 {
        select_single_table(blob, plan, limits)
    } else {
        select_join(blob, plan, limits)
    }
}

fn select_single_table(blob: &str, plan: &SelectPlan, limits: &Limits) -> Result<RowSet> {
    let source = plan
        .sources
        .first()
        .expect("a SELECT plan always has a root source");
    let layout = RowLayout {
        table: &source.table,
        columns: &source.schema,
    };
    let mut result_bytes = std::mem::size_of::<RowSet>();
    check_limit(result_bytes, limits.max_result_bytes, "result bytes")?;
    let columns = materialize_result_columns(plan, &mut result_bytes, limits)?;

    let mut rows = Vec::new();
    let row_structure = row_structure_charge(plan.projection.len(), limits)?;

    for matched in plan.regex.find_iter(blob) {
        let matched = matched.map_err(|error| map_regex_runtime(error, limits))?;
        let structural_total = result_bytes
            .checked_add(row_structure)
            .ok_or_else(|| result_limit_error(limits))?;
        check_limit(structural_total, limits.max_result_bytes, "result bytes")?;

        let decoded = storage::decode_row(matched.as_str(), layout)?;
        let payload_bytes = plan
            .projection
            .iter()
            .try_fold(0_usize, |total, location| {
                debug_assert_eq!(location.source, 0);
                total
                    .checked_add(value_payload_size(&decoded[location.column]))
                    .ok_or_else(|| result_limit_error(limits))
            })?;
        let row_charge = row_structure
            .checked_add(payload_bytes)
            .ok_or_else(|| result_limit_error(limits))?;
        charge_result(&mut result_bytes, row_charge, limits)?;

        rows.try_reserve(1)
            .map_err(|_| result_limit_error(limits))?;
        let mut row = Vec::new();
        row.try_reserve_exact(plan.projection.len())
            .map_err(|_| result_limit_error(limits))?;
        for location in &plan.projection {
            row.push(clone_result_value(&decoded[location.column], limits)?);
        }
        rows.push(row);
    }

    Ok(RowSet { columns, rows })
}

fn select_join(blob: &str, plan: &SelectPlan, limits: &Limits) -> Result<RowSet> {
    let mut result_bytes = std::mem::size_of::<RowSet>();
    check_limit(result_bytes, limits.max_result_bytes, "result bytes")?;
    let columns = materialize_result_columns(plan, &mut result_bytes, limits)?;

    let source_slots = plan
        .sources
        .len()
        .checked_mul(std::mem::size_of::<Vec<Vec<Value>>>())
        .ok_or_else(|| result_limit_error(limits))?;
    charge_result(&mut result_bytes, source_slots, limits)?;
    let mut source_rows = Vec::new();
    source_rows
        .try_reserve_exact(plan.sources.len())
        .map_err(|_| result_limit_error(limits))?;
    source_rows.resize_with(plan.sources.len(), Vec::new);

    for matched in plan.regex.find_iter(blob) {
        let matched = matched.map_err(|error| map_regex_runtime(error, limits))?;
        let table = storage::row_table(matched.as_str())?;
        let source_index = plan
            .sources
            .iter()
            .position(|source| source.table == table)
            .ok_or_else(|| Error::RegexRuntime(format!("matched unexpected table {table:?}")))?;
        let source = &plan.sources[source_index];
        let decoded = storage::decode_row(
            matched.as_str(),
            RowLayout {
                table: &source.table,
                columns: &source.schema,
            },
        )?;
        let structure = row_structure_charge(decoded.len(), limits)?;
        let payload = decoded.iter().try_fold(0_usize, |total, value| {
            total
                .checked_add(value_allocation_size(value))
                .ok_or_else(|| result_limit_error(limits))
        })?;
        let charge = structure
            .checked_add(payload)
            .ok_or_else(|| result_limit_error(limits))?;
        charge_result(&mut result_bytes, charge, limits)?;
        source_rows[source_index]
            .try_reserve(1)
            .map_err(|_| result_limit_error(limits))?;
        source_rows[source_index].push(decoded);
    }

    let chosen_slots = plan
        .sources
        .len()
        .checked_mul(std::mem::size_of::<&[Value]>())
        .ok_or_else(|| result_limit_error(limits))?;
    charge_result(&mut result_bytes, chosen_slots, limits)?;
    let mut chosen = Vec::new();
    chosen
        .try_reserve_exact(plan.sources.len())
        .map_err(|_| result_limit_error(limits))?;
    let mut rows = Vec::new();
    let row_structure = row_structure_charge(plan.projection.len(), limits)?;
    let mut output = JoinOutput {
        plan,
        rows: &mut rows,
        result_bytes: &mut result_bytes,
        join_steps: 0,
        row_structure,
        limits,
    };
    emit_join_rows(0, &mut chosen, &source_rows, &mut output)?;

    Ok(RowSet { columns, rows })
}

struct JoinOutput<'a> {
    plan: &'a SelectPlan,
    rows: &'a mut Vec<Vec<Value>>,
    result_bytes: &'a mut usize,
    join_steps: usize,
    row_structure: usize,
    limits: &'a Limits,
}

fn emit_join_rows<'rows>(
    source: usize,
    chosen: &mut Vec<&'rows [Value]>,
    source_rows: &'rows [Vec<Vec<Value>>],
    output: &mut JoinOutput<'_>,
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
                    .ok_or_else(|| result_limit_error(output.limits))
            })?;
        let charge = output
            .row_structure
            .checked_add(payload)
            .ok_or_else(|| result_limit_error(output.limits))?;
        charge_result(output.result_bytes, charge, output.limits)?;

        output
            .rows
            .try_reserve(1)
            .map_err(|_| result_limit_error(output.limits))?;
        let mut row = Vec::new();
        row.try_reserve_exact(output.plan.projection.len())
            .map_err(|_| result_limit_error(output.limits))?;
        for location in &output.plan.projection {
            row.push(clone_result_value(
                &chosen[location.source][location.column],
                output.limits,
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
                .ok_or(Error::ResourceLimit {
                resource: "JOIN execution steps",
                limit: limits.max_join_steps,
            })?,
            _ => 1,
        };
        *join_steps = join_steps
            .checked_add(comparison_cost)
            .ok_or(Error::ResourceLimit {
                resource: "JOIN execution steps",
                limit: limits.max_join_steps,
            })?;
        check_limit(*join_steps, limits.max_join_steps, "JOIN execution steps")?;
        if matches!(left, Value::Null) || matches!(right, Value::Null) || left != right {
            return Ok(false);
        }
    }
    Ok(true)
}

fn materialize_result_columns(
    plan: &SelectPlan,
    result_bytes: &mut usize,
    limits: &Limits,
) -> Result<Vec<Column>> {
    let column_slots = plan
        .projection
        .len()
        .checked_mul(std::mem::size_of::<Column>())
        .ok_or_else(|| result_limit_error(limits))?;
    charge_result(result_bytes, column_slots, limits)?;

    let mut columns = Vec::new();
    columns
        .try_reserve_exact(plan.projection.len())
        .map_err(|_| result_limit_error(limits))?;
    for location in &plan.projection {
        let column = &plan.sources[location.source].schema[location.column];
        charge_result(result_bytes, column.name.len(), limits)?;
        let mut name = String::new();
        name.try_reserve_exact(column.name.len())
            .map_err(|_| result_limit_error(limits))?;
        name.push_str(&column.name);
        columns.push(Column {
            name,
            data_type: column.data_type,
            nullable: column.nullable,
        });
    }
    Ok(columns)
}

fn row_structure_charge(column_count: usize, limits: &Limits) -> Result<usize> {
    let value_slots = column_count
        .checked_mul(std::mem::size_of::<Value>())
        .ok_or_else(|| result_limit_error(limits))?;
    // Vec growth may reserve more outer row slots than are immediately used. Charging
    // four row descriptors per row keeps the byte budget conservative.
    let row_descriptors = std::mem::size_of::<Vec<Value>>()
        .checked_mul(4)
        .ok_or_else(|| result_limit_error(limits))?;
    row_descriptors
        .checked_add(value_slots)
        .ok_or_else(|| result_limit_error(limits))
}

pub(super) fn rewrite_matching_rows<F>(
    blob: &str,
    plan: &ScanPlan,
    limits: &Limits,
    mut rewrite: F,
) -> Result<(String, usize)>
where
    F: FnMut(Vec<Value>) -> Result<Option<Vec<Value>>>,
{
    let layout = RowLayout {
        table: &plan.table,
        columns: &plan.schema,
    };
    let mut candidate = storage::Candidate::new(blob, limits.max_database_bytes)?;
    let mut affected = 0_usize;

    for matched in plan.regex.find_iter(blob) {
        let matched = matched.map_err(|error| map_regex_runtime(error, limits))?;
        let values = storage::decode_row(matched.as_str(), layout)?;
        let replacement = rewrite(values)?;
        candidate.rewrite_row(
            matched.start()..matched.end(),
            layout,
            replacement.as_deref(),
        )?;
        affected = affected.checked_add(1).ok_or(Error::ResourceLimit {
            resource: "affected rows",
            limit: usize::MAX,
        })?;
    }
    Ok((candidate.finish()?, affected))
}

fn value_payload_size(value: &Value) -> usize {
    match value {
        Value::Text(value) => value.len(),
        Value::Integer(_) | Value::Boolean(_) | Value::Null => 0,
    }
}

fn value_allocation_size(value: &Value) -> usize {
    match value {
        Value::Text(value) => value.capacity(),
        Value::Integer(_) | Value::Boolean(_) | Value::Null => 0,
    }
}

fn clone_result_value(value: &Value, limits: &Limits) -> Result<Value> {
    match value {
        Value::Text(value) => {
            let mut cloned = String::new();
            cloned
                .try_reserve_exact(value.len())
                .map_err(|_| result_limit_error(limits))?;
            cloned.push_str(value);
            Ok(Value::Text(cloned))
        }
        Value::Integer(value) => Ok(Value::Integer(*value)),
        Value::Boolean(value) => Ok(Value::Boolean(*value)),
        Value::Null => Ok(Value::Null),
    }
}

fn charge_result(total: &mut usize, amount: usize, limits: &Limits) -> Result<()> {
    *total = total
        .checked_add(amount)
        .ok_or_else(|| result_limit_error(limits))?;
    check_limit(*total, limits.max_result_bytes, "result bytes")
}

fn result_limit_error(limits: &Limits) -> Error {
    Error::ResourceLimit {
        resource: "result bytes",
        limit: limits.max_result_bytes,
    }
}

fn map_regex_runtime(error: FancyError, limits: &Limits) -> Error {
    match error {
        FancyError::RuntimeError(
            RuntimeError::BacktrackLimitExceeded | RuntimeError::StackOverflow,
        ) => Error::ResourceLimit {
            resource: "regex execution steps",
            limit: limits.regex_backtrack_limit,
        },
        other => Error::RegexRuntime(other.to_string()),
    }
}
