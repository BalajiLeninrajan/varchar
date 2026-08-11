//! Canonical decoding of flat persisted CHECK programs.

mod syntax;

use super::super::super::TableSchema;
use super::super::super::budget::WorkingBudget;
use super::super::super::decode::CheckMetadata;
use super::super::super::format::corrupt;
use crate::expression::{CheckProgram, CheckProgramNode};
use crate::limits::check_limit;
use crate::{Error, Resource, Result};
use syntax::{Fields, decode_node, validate_node};

#[cfg(test)]
use syntax::{Field, decode_usize};

struct StructuralProgram {
    node_count: usize,
    predicate_units: usize,
}

pub(super) fn decode_program(
    schema: &TableSchema,
    metadata: CheckMetadata<'_>,
    existing_predicates: usize,
    max_predicates: usize,
    budget: &mut WorkingBudget,
) -> Result<(CheckProgram, usize)> {
    let structural = prevalidate_structure(schema, metadata.program, metadata.program_offset)?;
    let predicates = existing_predicates
        .checked_add(structural.predicate_units)
        .ok_or(Error::Capacity {
            operation: "counting table CHECK predicates",
        })?;
    check_limit(predicates, max_predicates, Resource::CheckPredicates)?;

    let mut fields = Fields::new(metadata.program, metadata.program_offset);
    let mut nodes: Vec<CheckProgramNode> = Vec::new();
    budget.reserve_exact(
        &mut nodes,
        structural.node_count,
        "reserving decoded CHECK nodes",
    )?;
    while let Some(opcode) = fields.next() {
        let (node, _) = decode_node(schema, opcode, &mut fields, budget)?;
        nodes.push(node);
    }
    Ok((CheckProgram::new(nodes), predicates))
}

fn prevalidate_structure(
    schema: &TableSchema,
    program: &str,
    program_offset: usize,
) -> Result<StructuralProgram> {
    let mut fields = Fields::new(program, program_offset);
    let mut pending = 1_usize;
    let mut node_count = 0_usize;
    let mut predicate_units = 0_usize;
    let mut saw_root = false;

    while let Some(opcode) = fields.next() {
        if pending == 0 {
            return Err(corrupt(
                opcode.offset,
                "CHECK program contains trailing nodes or fields",
            ));
        }
        saw_root = true;
        pending -= 1;
        node_count = node_count.checked_add(1).ok_or(Error::Capacity {
            operation: "counting CHECK program nodes",
        })?;

        let validated = validate_node(schema, opcode, &mut fields)?;
        predicate_units = predicate_units
            .checked_add(validated.predicate_units)
            .ok_or(Error::Capacity {
                operation: "counting CHECK predicates",
            })?;
        if let Some((_, children, count_offset)) = validated.logical {
            if children < 2 {
                return Err(corrupt(
                    count_offset,
                    "CHECK AND/OR nodes require at least two children",
                ));
            }
            pending = pending.checked_add(children).ok_or_else(|| {
                corrupt(count_offset, "CHECK child count exceeds program capacity")
            })?;
        }
    }

    if !saw_root {
        return Err(corrupt(
            fields.end_offset(),
            "CHECK metadata is missing its program",
        ));
    }
    if pending != 0 {
        return Err(corrupt(
            fields.end_offset(),
            "CHECK program ends before all children are encoded",
        ));
    }
    Ok(StructuralProgram {
        node_count,
        predicate_units,
    })
}

#[cfg(test)]
mod tests;
