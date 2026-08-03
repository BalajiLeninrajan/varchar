//! Bounded row decoding and iterative CHECK evaluation.

use super::{ValidationError, ValidationResult, Violation};
use crate::expression::CheckEvaluator;
use crate::limits::ByteBudget;
use crate::storage::decode::{decode_cell_at, row_records};
use crate::storage::format::{ROW_PREFIX, corrupt};
use crate::storage::{Catalog, TableSchema};
use crate::{DataType, Error, Result, Value};

pub(super) fn validate(
    blob: &str,
    catalog: &Catalog,
    budget: &mut ByteBudget,
    like_work_limit: usize,
) -> ValidationResult<()> {
    let Some((column_capacity, logical_capacity)) = workspace_shape(catalog) else {
        return Ok(());
    };

    let row_bytes = column_capacity
        .checked_mul(std::mem::size_of::<Value>())
        .ok_or(Error::Capacity {
            operation: "sizing decoded CHECK row values",
        })?;
    let evaluator_bytes = CheckEvaluator::working_bytes(logical_capacity)?;
    let workspace_bytes = row_bytes
        .checked_add(evaluator_bytes)
        .ok_or(Error::Capacity {
            operation: "sizing CHECK validation state",
        })?;

    let mut values = Vec::new();
    budget.reserve_exact(
        &mut values,
        column_capacity,
        "reserving decoded CHECK row values",
    )?;
    budget.charge(evaluator_bytes)?;
    let mut evaluator =
        CheckEvaluator::new_with_like_work_limit(logical_capacity, like_work_limit)?;

    let result = validate_rows(blob, catalog, budget, &mut values, &mut evaluator);
    drop(evaluator);
    drop(values);
    budget.release(workspace_bytes);
    result
}

fn workspace_shape(catalog: &Catalog) -> Option<(usize, usize)> {
    let mut column_capacity = 0_usize;
    let mut logical_capacity = 0_usize;
    let mut has_checks = false;
    for schema in catalog
        .tables
        .values()
        .filter(|schema| !schema.checks.is_empty())
    {
        has_checks = true;
        column_capacity = column_capacity.max(schema.columns.len());
        for check in &schema.checks {
            logical_capacity = logical_capacity.max(check.logical_node_count());
        }
    }
    has_checks.then_some((column_capacity, logical_capacity))
}

fn validate_rows(
    blob: &str,
    catalog: &Catalog,
    budget: &mut ByteBudget,
    values: &mut Vec<Value>,
    evaluator: &mut CheckEvaluator,
) -> ValidationResult<()> {
    for row in row_records(blob, catalog.row_start) {
        let row = row.map_err(ValidationError::Storage)?;
        let Some(schema) = catalog.tables.get(row.table()) else {
            return Err(Violation::new(
                row.range().start,
                "row table disappeared during CHECK validation",
            )
            .into());
        };
        if schema.checks.is_empty() {
            continue;
        }

        let text_bytes =
            decode_row_values(&row, schema, values, budget).map_err(ValidationError::Storage)?;
        let result = evaluate_checks(evaluator, schema, values, row.range().start);
        values.clear();
        budget.release(text_bytes);
        result?;
    }
    Ok(())
}

fn decode_row_values(
    row: &crate::storage::decode::RowRecordRef<'_>,
    schema: &TableSchema,
    values: &mut Vec<Value>,
    budget: &mut ByteBudget,
) -> Result<usize> {
    values.clear();
    let mut text_bytes = 0_usize;
    let mut cells = row.cells();
    let mut offset = row.range().start + ROW_PREFIX.len() + row.table().len() + 1;

    for column in &schema.columns {
        let Some(encoded) = cells.next() else {
            release_decoded_values(values, text_bytes, budget);
            return Err(corrupt(
                row.range().start,
                "row ended during CHECK decoding",
            ));
        };
        let allocation = text_allocation(encoded, column.data_type);
        if let Err(error) = budget.charge(allocation) {
            release_decoded_values(values, text_bytes, budget);
            return Err(error);
        }
        text_bytes = match text_bytes.checked_add(allocation) {
            Some(bytes) => bytes,
            None => {
                budget.release(allocation);
                release_decoded_values(values, text_bytes, budget);
                return Err(Error::Capacity {
                    operation: "counting decoded CHECK row text",
                });
            }
        };
        match decode_cell_at(encoded, column, offset) {
            Ok(value) => values.push(value),
            Err(error) => {
                release_decoded_values(values, text_bytes, budget);
                return Err(error);
            }
        }
        offset += encoded.len() + 1;
    }
    if cells.next().is_some() {
        release_decoded_values(values, text_bytes, budget);
        return Err(corrupt(
            row.range().start,
            "row has trailing cells during CHECK decoding",
        ));
    }
    Ok(text_bytes)
}

fn evaluate_checks(
    evaluator: &mut CheckEvaluator,
    schema: &TableSchema,
    values: &[Value],
    offset: usize,
) -> ValidationResult<()> {
    for (index, check) in schema.checks.iter().enumerate() {
        let passes = evaluator
            .evaluate(check, values)
            .map_err(ValidationError::Storage)?;
        if !passes {
            return Err(Violation::new(
                offset,
                format!(
                    "CHECK constraint {} failed for table {:?}",
                    index + 1,
                    schema.name
                ),
            )
            .into());
        }
    }
    Ok(())
}

fn text_allocation(encoded: &str, data_type: DataType) -> usize {
    if data_type == DataType::Text {
        encoded.strip_prefix('T').map_or(0, str::len)
    } else {
        0
    }
}

fn release_decoded_values(values: &mut Vec<Value>, text_bytes: usize, budget: &mut ByteBudget) {
    values.clear();
    budget.release(text_bytes);
}

#[cfg(test)]
mod tests;
