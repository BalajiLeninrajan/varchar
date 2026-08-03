//! Bounded selection, joins, explanations, and result materialization.

mod collector;

use super::map_regex_runtime;
use crate::expression::{Evaluator, Program};
use crate::limits::{Limits, check_limit};
use crate::output::{RowSet, SelectExplanation};
use crate::resolve::ResolvedJoinCondition;
use crate::storage::{self, RowRecordRef};
use crate::value::Value;
use crate::{Error, Resource, Result};

use self::collector::{CollectionStatus, RowCollector, materialize_result_columns};
use super::super::SelectPlan;

pub(crate) fn select(blob: &str, plan: &SelectPlan<'_, '_>, limits: &Limits) -> Result<RowSet> {
    if plan.sources.len() == 1 {
        select_single_table(blob, plan, limits)
    } else {
        select_join(blob, plan, limits)
    }
}

pub(crate) fn explain(
    plan: SelectPlan<'_, '_>,
    max_query_output_bytes: usize,
) -> Result<SelectExplanation> {
    let mut output_budget = ByteBudget::new(max_query_output_bytes, Resource::QueryOutputBytes);
    output_budget.charge(std::mem::size_of::<SelectExplanation>())?;
    output_budget.charge(plan.pattern.len())?;

    let source_slots = plan
        .sources
        .len()
        .checked_mul(std::mem::size_of::<String>())
        .ok_or_else(|| output_budget.limit_error())?;
    output_budget.charge(source_slots)?;
    let mut sources = Vec::new();
    sources
        .try_reserve_exact(plan.sources.len())
        .map_err(|_| allocation_error("reserving explanation sources"))?;
    for source in &plan.sources {
        output_budget.charge(source.name.len())?;
        sources.push(clone_explanation_string(&source.name)?);
    }

    let columns = materialize_result_columns(&plan, &mut output_budget)?;
    // The pattern expresses the whole `WHERE` clause only when nothing was left
    // for Rust-side row filtering. A multi-source pattern is an alternation over
    // whole source rows and never encodes the `ON` conditions that the nested
    // loops apply, so it can only ever prefilter.
    let pattern_is_exact = plan.sources.len() == 1
        && plan.local_residuals.iter().all(Option::is_none)
        && plan.cross_source_residual.is_none();
    Ok(SelectExplanation::new(
        plan.pattern,
        pattern_is_exact,
        sources,
        columns,
    ))
}

fn select_single_table(blob: &str, plan: &SelectPlan<'_, '_>, limits: &Limits) -> Result<RowSet> {
    let source = plan
        .sources
        .first()
        .expect("a SELECT plan always has a root source");
    let layout = source.row_layout();
    let mut output_budget =
        ByteBudget::new(limits.max_query_output_bytes, Resource::QueryOutputBytes);
    output_budget.charge(std::mem::size_of::<RowSet>())?;
    let columns = materialize_result_columns(plan, &mut output_budget)?;
    let mut collector = RowCollector::new(plan, output_budget)?;
    if !collector.should_scan() {
        return collector.finish(columns);
    }
    let mut working_budget =
        ByteBudget::new(limits.max_query_working_bytes, Resource::QueryWorkingBytes);

    let local_residual = plan.local_residuals.first().ok_or(Error::Capacity {
        operation: "reading a single-table residual program",
    })?;
    let mut evaluator = residual_evaluator(
        std::slice::from_ref(local_residual),
        None,
        &mut working_budget,
        limits.regex_backtrack_limit,
    )?;

    for matched in plan.regex.find_iter(blob) {
        let matched = matched.map_err(|error| map_regex_runtime(error, limits))?;
        let row_record = storage::row_record(matched.as_str(), matched.start())?;
        let decoded_charge =
            decoded_row_charge(&row_record, source.columns.len(), &working_budget)?;
        working_budget.check_transient(decoded_charge)?;

        let decoded = storage::decode_row(matched.as_str(), layout)?;
        if let (Some(program), Some(evaluator)) = (local_residual, &mut evaluator)
            && !evaluator.evaluate_where_local(program, 0, &decoded)?
        {
            continue;
        }
        let selected = [decoded.as_slice()];
        if collector.collect(&selected, &mut working_budget, decoded_charge)?
            == CollectionStatus::Complete
        {
            break;
        }
    }

    collector.finish(columns)
}

fn select_join(blob: &str, plan: &SelectPlan<'_, '_>, limits: &Limits) -> Result<RowSet> {
    let mut output_budget =
        ByteBudget::new(limits.max_query_output_bytes, Resource::QueryOutputBytes);
    output_budget.charge(std::mem::size_of::<RowSet>())?;
    let columns = materialize_result_columns(plan, &mut output_budget)?;
    let mut collector = RowCollector::new(plan, output_budget)?;
    if !collector.should_scan() {
        return collector.finish(columns);
    }

    let mut working_budget =
        ByteBudget::new(limits.max_query_working_bytes, Resource::QueryWorkingBytes);
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

    let mut evaluator = residual_evaluator(
        &plan.local_residuals,
        plan.cross_source_residual.as_ref(),
        &mut working_budget,
        limits.regex_backtrack_limit,
    )?;

    for matched in plan.regex.find_iter(blob) {
        let matched = matched.map_err(|error| map_regex_runtime(error, limits))?;
        let row_record = storage::row_record(matched.as_str(), matched.start())?;
        let table = row_record.table();
        let source_index = plan
            .sources
            .iter()
            .position(|source| source.name == table)
            .ok_or_else(|| Error::RegexRuntime(format!("matched unexpected table {table:?}")))?;
        let source = plan.sources[source_index];
        working_budget.check_transient(decoded_row_charge(
            &row_record,
            source.columns.len(),
            &working_budget,
        )?)?;
        let decoded = storage::decode_row(matched.as_str(), source.row_layout())?;
        let residual = plan
            .local_residuals
            .get(source_index)
            .ok_or(Error::Capacity {
                operation: "reading a JOIN source-local residual program",
            })?;
        if let Some(program) = residual {
            let evaluator = evaluator.as_mut().ok_or(Error::Capacity {
                operation: "reading the reusable JOIN residual evaluator",
            })?;
            if !evaluator.evaluate_where_local(program, source_index, &decoded)? {
                continue;
            }
        }

        let retained_row_charge =
            retained_row_charge(&row_record, source.columns.len(), &working_budget)?;
        working_budget.charge(retained_row_charge)?;
        source_rows[source_index]
            .try_reserve(1)
            .map_err(|_| allocation_error("retaining decoded JOIN rows"))?;
        source_rows[source_index].push(decoded);
    }

    let residual_evaluator = if plan.cross_source_residual.is_some() {
        evaluator
    } else {
        None
    };
    let mut output = JoinOutput {
        plan,
        collector: &mut collector,
        working_budget: &mut working_budget,
        residual_evaluator,
        join_steps: 0,
        limits,
    };
    emit_join_rows(0, &mut chosen, &source_rows, &mut output)?;

    collector.finish(columns)
}

struct JoinOutput<'a, 'plan, 'catalog, 'statement, 'limits> {
    plan: &'plan SelectPlan<'catalog, 'statement>,
    collector: &'a mut RowCollector<'plan>,
    working_budget: &'a mut ByteBudget,
    residual_evaluator: Option<Evaluator>,
    join_steps: usize,
    limits: &'limits Limits,
}

fn emit_join_rows<'rows>(
    source: usize,
    chosen: &mut Vec<&'rows [Value]>,
    source_rows: &'rows [Vec<Vec<Value>>],
    output: &mut JoinOutput<'_, '_, '_, '_, '_>,
) -> Result<CollectionStatus> {
    if source == source_rows.len() {
        if let (Some(program), Some(evaluator)) = (
            &output.plan.cross_source_residual,
            &mut output.residual_evaluator,
        ) && !evaluator.evaluate_where(program, chosen)?
        {
            return Ok(CollectionStatus::Continue);
        }
        return output.collector.collect(chosen, output.working_budget, 0);
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
        let status = if matches {
            emit_join_rows(source + 1, chosen, source_rows, output)?
        } else {
            CollectionStatus::Continue
        };
        chosen.pop();
        if status == CollectionStatus::Complete {
            return Ok(status);
        }
    }
    Ok(CollectionStatus::Continue)
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
        check_limit(*join_steps, limits.max_join_steps, Resource::JoinSteps)?;
        if matches!(left, Value::Null) || matches!(right, Value::Null) || left != right {
            return Ok(false);
        }
    }
    Ok(true)
}

fn residual_evaluator(
    local_residuals: &[Option<Program<'_>>],
    cross_source_residual: Option<&Program<'_>>,
    working_budget: &mut ByteBudget,
    like_work_limit: usize,
) -> Result<Option<Evaluator>> {
    let mut largest = None;
    for program in local_residuals
        .iter()
        .filter_map(Option::as_ref)
        .chain(cross_source_residual)
    {
        let bytes = Evaluator::working_bytes(program)?;
        if largest.is_none_or(|(_, largest_bytes)| bytes > largest_bytes) {
            largest = Some((program, bytes));
        }
    }

    let Some((program, bytes)) = largest else {
        return Ok(None);
    };
    working_budget.charge(bytes)?;
    Evaluator::new(program, like_work_limit).map(Some)
}

fn decoded_row_charge(
    row: &RowRecordRef<'_>,
    column_count: usize,
    budget: &ByteBudget,
) -> Result<usize> {
    let value_slots = column_count
        .checked_mul(std::mem::size_of::<Value>())
        .ok_or_else(|| budget.limit_error())?;
    std::mem::size_of::<Vec<Value>>()
        .checked_add(value_slots)
        .ok_or_else(|| budget.limit_error())?
        .checked_add(row.range().len())
        .ok_or_else(|| budget.limit_error())
}

fn retained_row_charge(
    row: &RowRecordRef<'_>,
    column_count: usize,
    budget: &ByteBudget,
) -> Result<usize> {
    // The outer source bucket grows geometrically. Charge four descriptors per
    // logical row, matching the conservative accounting used for output rows.
    let spare_descriptors = std::mem::size_of::<Vec<Value>>()
        .checked_mul(3)
        .ok_or_else(|| budget.limit_error())?;
    decoded_row_charge(row, column_count, budget)?
        .checked_add(spare_descriptors)
        .ok_or_else(|| budget.limit_error())
}

fn clone_explanation_string(value: &str) -> Result<String> {
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

    fn charge_with_transient(&mut self, amount: usize, transient: usize) -> Result<()> {
        let next = self
            .used
            .checked_add(amount)
            .ok_or_else(|| self.limit_error())?;
        let peak = next
            .checked_add(transient)
            .ok_or_else(|| self.limit_error())?;
        check_limit(peak, self.limit, self.resource)?;
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
        limit: limits.max_join_steps,
    }
}

#[cfg(test)]
mod tests;
