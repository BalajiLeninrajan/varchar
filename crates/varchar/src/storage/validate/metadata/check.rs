//! Canonical decoding of flat persisted CHECK programs.

mod syntax;

use super::super::super::TableSchema;
use super::super::super::decode::CheckMetadata;
use super::super::super::format::corrupt;
use crate::expression::{CheckProgram, CheckProgramNode};
use crate::limits::ByteBudget;
use crate::limits::check_limit;
use crate::{Error, Resource, Result};
use syntax::{Fields, LogicalOperator, decode_node, validate_node};

#[cfg(test)]
use syntax::{Field, decode_usize};

struct ShapeFrame {
    operator: LogicalOperator,
    remaining: usize,
}

struct PrevalidatedProgram {
    node_count: usize,
    predicate_units: usize,
    deferred_working_error: Option<Error>,
}

struct StructuralProgram {
    node_count: usize,
    predicate_units: usize,
}

pub(super) fn decode_program(
    schema: &TableSchema,
    metadata: CheckMetadata<'_>,
    existing_predicates: usize,
    max_predicates: usize,
    budget: &mut ByteBudget,
) -> Result<(CheckProgram, usize)> {
    let prevalidated =
        prevalidate_program(schema, metadata.program, metadata.program_offset, budget)?;
    let predicates = existing_predicates
        .checked_add(prevalidated.predicate_units)
        .ok_or(Error::Capacity {
            operation: "counting table CHECK predicates",
        })?;
    check_limit(predicates, max_predicates, Resource::CheckPredicates)?;
    if let Some(error) = prevalidated.deferred_working_error {
        return Err(error);
    }

    let mut fields = Fields::new(metadata.program, metadata.program_offset);
    let mut nodes = Vec::new();
    budget.reserve_exact(
        &mut nodes,
        prevalidated.node_count,
        "reserving decoded CHECK nodes",
    )?;
    let mut shape = Vec::new();
    let mut charged_shape_frames = 0_usize;
    let result = decode_program_inner(
        schema,
        &mut fields,
        &mut nodes,
        &mut shape,
        &mut charged_shape_frames,
        budget,
    );
    let shape_bytes = charged_shape_frames
        .checked_mul(std::mem::size_of::<ShapeFrame>())
        .ok_or(Error::Capacity {
            operation: "releasing CHECK shape validation state",
        })?;
    drop(shape);
    budget.release(shape_bytes);
    result.map(|()| (CheckProgram::new(nodes), predicates))
}

fn prevalidate_program(
    schema: &TableSchema,
    program: &str,
    program_offset: usize,
    budget: &mut ByteBudget,
) -> Result<PrevalidatedProgram> {
    let structural = match prevalidate_structure(schema, program, program_offset) {
        Ok(structural) => structural,
        Err(structural_error) => {
            let Error::CorruptStorage {
                offset: structural_offset,
                ..
            } = &structural_error
            else {
                return Err(structural_error);
            };
            let structural_offset = *structural_offset;
            return match linear_noncanonical_nesting(schema, program, program_offset, budget) {
                Ok(Some(nested_offset)) if nested_offset < structural_offset => {
                    Err(noncanonical_nesting(nested_offset))
                }
                Ok(_) => Err(structural_error),
                Err(error) if can_defer_prevalidation_error(&error) => Err(structural_error),
                Err(error @ Error::CorruptStorage { offset, .. }) if offset < structural_offset => {
                    Err(error)
                }
                Err(Error::CorruptStorage { .. }) => Err(structural_error),
                Err(error) => Err(error),
            };
        }
    };
    let deferred_working_error =
        match linear_noncanonical_nesting(schema, program, program_offset, budget) {
            Ok(Some(offset)) => return Err(noncanonical_nesting(offset)),
            Ok(None) => None,
            Err(error) if can_defer_prevalidation_error(&error) => Some(error),
            Err(error) => return Err(error),
        };
    Ok(PrevalidatedProgram {
        node_count: structural.node_count,
        predicate_units: structural.predicate_units,
        deferred_working_error,
    })
}

fn noncanonical_nesting(offset: usize) -> Error {
    corrupt(
        offset,
        "CHECK program contains a noncanonical nested associative node",
    )
}

fn can_defer_prevalidation_error(error: &Error) -> bool {
    matches!(
        error,
        Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            ..
        } | Error::Allocation { .. }
    )
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

fn linear_noncanonical_nesting(
    schema: &TableSchema,
    program: &str,
    program_offset: usize,
    budget: &mut ByteBudget,
) -> Result<Option<usize>> {
    let mut fields = Fields::new(program, program_offset);
    let mut shape = Vec::new();
    let mut charged_shape_frames = 0_usize;
    let result = linear_noncanonical_nesting_inner(
        schema,
        &mut fields,
        &mut shape,
        &mut charged_shape_frames,
        budget,
    );
    let shape_bytes = charged_shape_frames
        .checked_mul(std::mem::size_of::<ShapeFrame>())
        .ok_or(Error::Capacity {
            operation: "releasing CHECK prevalidation state",
        })?;
    drop(shape);
    budget.release(shape_bytes);
    result
}

fn linear_noncanonical_nesting_inner(
    schema: &TableSchema,
    fields: &mut Fields<'_>,
    shape: &mut Vec<ShapeFrame>,
    charged_shape_frames: &mut usize,
    budget: &mut ByteBudget,
) -> Result<Option<usize>> {
    let mut saw_root = false;
    while let Some(opcode) = fields.next() {
        if saw_root {
            let parent = shape.last_mut().ok_or_else(|| {
                corrupt(
                    opcode.offset,
                    "CHECK program contains trailing nodes or fields",
                )
            })?;
            parent.remaining = parent.remaining.checked_sub(1).ok_or_else(|| {
                corrupt(opcode.offset, "CHECK parent has too many encoded children")
            })?;
        } else {
            saw_root = true;
        }

        let validated = validate_node(schema, opcode, fields)?;
        if let Some((operator, children, count_offset)) = validated.logical {
            if children < 2 {
                return Err(corrupt(
                    count_offset,
                    "CHECK AND/OR nodes require at least two children",
                ));
            }
            if shape
                .last()
                .is_some_and(|parent| parent.operator == operator)
            {
                return Ok(Some(opcode.offset));
            }
            reserve_shape_frame(shape, charged_shape_frames, budget)?;
            shape.push(ShapeFrame {
                operator,
                remaining: children,
            });
        } else {
            while shape.last().is_some_and(|frame| frame.remaining == 0) {
                shape.pop();
            }
        }
    }

    if !saw_root {
        return Err(corrupt(
            fields.end_offset(),
            "CHECK metadata is missing its program",
        ));
    }
    if !shape.is_empty() {
        return Err(corrupt(
            fields.end_offset(),
            "CHECK program ends before all children are encoded",
        ));
    }
    Ok(None)
}

fn decode_program_inner(
    schema: &TableSchema,
    fields: &mut Fields<'_>,
    nodes: &mut Vec<CheckProgramNode>,
    shape: &mut Vec<ShapeFrame>,
    charged_shape_frames: &mut usize,
    budget: &mut ByteBudget,
) -> Result<()> {
    let mut saw_root = false;
    while let Some(opcode) = fields.next() {
        if saw_root && shape.is_empty() {
            return Err(corrupt(
                opcode.offset,
                "CHECK program contains trailing nodes or fields",
            ));
        }
        if !saw_root {
            saw_root = true;
        } else {
            let parent = shape.last_mut().ok_or_else(|| {
                corrupt(
                    opcode.offset,
                    "CHECK program contains more than one root expression",
                )
            })?;
            parent.remaining = parent.remaining.checked_sub(1).ok_or_else(|| {
                corrupt(opcode.offset, "CHECK parent has too many encoded children")
            })?;
        }

        let (node, logical) = decode_node(schema, opcode, fields, budget)?;
        nodes.push(node);

        if let Some((operator, children, count_offset)) = logical {
            if children < 2 {
                return Err(corrupt(
                    count_offset,
                    "CHECK AND/OR nodes require at least two children",
                ));
            }
            if shape
                .last()
                .is_some_and(|parent| parent.operator == operator)
            {
                return Err(corrupt(
                    opcode.offset,
                    "CHECK program contains a noncanonical nested associative node",
                ));
            }
            reserve_shape_frame(shape, charged_shape_frames, budget)?;
            shape.push(ShapeFrame {
                operator,
                remaining: children,
            });
        } else {
            while shape.last().is_some_and(|frame| frame.remaining == 0) {
                shape.pop();
            }
        }
    }

    if !saw_root {
        return Err(corrupt(
            fields.end_offset(),
            "CHECK metadata is missing its program",
        ));
    }
    if !shape.is_empty() {
        return Err(corrupt(
            fields.end_offset(),
            "CHECK program ends before all children are encoded",
        ));
    }
    Ok(())
}

fn reserve_shape_frame(
    shape: &mut Vec<ShapeFrame>,
    charged_shape_frames: &mut usize,
    budget: &mut ByteBudget,
) -> Result<()> {
    if shape.len() < *charged_shape_frames {
        return Ok(());
    }
    budget.charge_items::<ShapeFrame>(1)?;
    *charged_shape_frames = charged_shape_frames.checked_add(1).ok_or(Error::Capacity {
        operation: "counting CHECK shape validation frames",
    })?;
    shape.try_reserve(1).map_err(|_| Error::Allocation {
        operation: "reserving CHECK shape validation state",
    })
}

#[cfg(test)]
mod tests;
