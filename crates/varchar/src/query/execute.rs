//! Execution of compiled scans and bounded row materialization or rewriting.

use fancy_regex::{Error as FancyError, RuntimeError};

use super::{ScanPlan, SelectPlan};
use crate::limits::{Limits, check_limit};
use crate::storage::{self, Candidate, RowLayout};
use crate::{Column, Error, Result, RowSet, Value};

pub(super) fn select(blob: &str, plan: &SelectPlan, limits: &Limits) -> Result<RowSet> {
    let scan = &plan.scan;
    let layout = RowLayout {
        table: &scan.table,
        columns: &scan.schema,
    };
    let mut result_bytes = std::mem::size_of::<RowSet>();
    check_limit(result_bytes, limits.max_result_bytes, "result bytes")?;

    let column_slots = plan
        .projection
        .len()
        .checked_mul(std::mem::size_of::<Column>())
        .ok_or_else(|| result_limit_error(limits))?;
    charge_result(&mut result_bytes, column_slots, limits)?;

    let mut columns = Vec::new();
    columns
        .try_reserve_exact(plan.projection.len())
        .map_err(|_| result_limit_error(limits))?;
    for &index in &plan.projection {
        let column = &scan.schema[index];
        charge_result(&mut result_bytes, column.name.len(), limits)?;
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

    let mut rows = Vec::new();
    let value_slots = plan
        .projection
        .len()
        .checked_mul(std::mem::size_of::<Value>())
        .ok_or_else(|| result_limit_error(limits))?;
    // Vec growth may reserve more outer row slots than are immediately used. Charging
    // four row descriptors per returned row keeps the byte budget conservative.
    let row_descriptors = std::mem::size_of::<Vec<Value>>()
        .checked_mul(4)
        .ok_or_else(|| result_limit_error(limits))?;
    let row_structure = row_descriptors
        .checked_add(value_slots)
        .ok_or_else(|| result_limit_error(limits))?;

    for matched in scan.regex.find_iter(blob) {
        let matched = matched.map_err(|error| map_regex_runtime(error, limits))?;
        let structural_total = result_bytes
            .checked_add(row_structure)
            .ok_or_else(|| result_limit_error(limits))?;
        check_limit(structural_total, limits.max_result_bytes, "result bytes")?;

        let decoded = storage::decode_row(matched.as_str(), layout)?;
        let payload_bytes = plan.projection.iter().try_fold(0_usize, |total, &index| {
            total
                .checked_add(value_payload_size(&decoded[index]))
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
        for &index in &plan.projection {
            row.push(clone_result_value(&decoded[index], limits)?);
        }
        rows.push(row);
    }

    Ok(RowSet { columns, rows })
}

pub(super) fn rewrite_matching_rows<F>(
    candidate: &mut Candidate<'_>,
    plan: &ScanPlan,
    limits: &Limits,
    mut rewrite: F,
) -> Result<usize>
where
    F: FnMut(Vec<Value>) -> Result<Option<Vec<Value>>>,
{
    let layout = RowLayout {
        table: &plan.table,
        columns: &plan.schema,
    };
    let mut affected = 0_usize;
    let blob = candidate.source();

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
    Ok(affected)
}

fn value_payload_size(value: &Value) -> usize {
    match value {
        Value::Text(value) => value.len(),
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
