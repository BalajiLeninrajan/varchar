use crate::expression::{CheckPredicate, CheckProgramNode, LikeAtom};
use crate::storage::TableSchema;
use crate::storage::budget::WorkingBudget;
use crate::storage::decode::{decode_check_value_at, validate_check_value_at};
use crate::storage::format::{corrupt, scan_text};
use crate::{DataType, Result};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LogicalOperator {
    And,
    Or,
}

pub(super) type LogicalNode = Option<(LogicalOperator, usize, usize)>;

pub(super) struct ValidatedNode {
    pub(super) logical: LogicalNode,
    pub(super) predicate_units: usize,
}

pub(super) fn decode_node(
    schema: &TableSchema,
    opcode: Field<'_>,
    fields: &mut Fields<'_>,
    budget: &mut WorkingBudget,
) -> Result<(CheckProgramNode, LogicalNode)> {
    match opcode.text {
        "AND" | "OR" => {
            let count = fields.required("CHECK logical node is missing its child count")?;
            let children = decode_usize(count, "invalid CHECK child count")?;
            let operator = if opcode.text == "AND" {
                LogicalOperator::And
            } else {
                LogicalOperator::Or
            };
            let node = if operator == LogicalOperator::And {
                CheckProgramNode::And { children }
            } else {
                CheckProgramNode::Or { children }
            };
            Ok((node, Some((operator, children, count.offset))))
        }
        "EQ" | "NE" | "LT" | "LE" | "GT" | "GE" => {
            let (column, definition) = decode_column(schema, fields)?;
            let operand = fields.required("CHECK comparison is missing its operand")?;
            if operand.text == "N" {
                return Err(corrupt(
                    operand.offset,
                    "CHECK comparison operands cannot be NULL",
                ));
            }
            validate_check_value_at(operand.text, definition.data_type, operand.offset)?;
            charge_text_operand(definition.data_type, operand.text, operand.offset, budget)?;
            let value = decode_check_value_at(operand.text, definition.data_type, operand.offset)?;
            let predicate = match opcode.text {
                "EQ" => CheckPredicate::Equal { column, value },
                "NE" => CheckPredicate::NotEqual { column, value },
                "LT" => CheckPredicate::LessThan { column, value },
                "LE" => CheckPredicate::LessThanOrEqual { column, value },
                "GT" => CheckPredicate::GreaterThan { column, value },
                "GE" => CheckPredicate::GreaterThanOrEqual { column, value },
                _ => unreachable!("the opcode match is exhaustive"),
            };
            Ok((CheckProgramNode::Predicate(predicate), None))
        }
        "LIKE" => {
            let (column, definition) = decode_column(schema, fields)?;
            if definition.data_type != DataType::Text {
                return Err(corrupt(opcode.offset, "CHECK LIKE requires a TEXT column"));
            }
            let count = fields.required("CHECK LIKE is missing its atom count")?;
            let atom_count = decode_usize(count, "invalid CHECK LIKE atom count")?;
            let mut preview = fields.clone();
            for _ in 0..atom_count {
                decode_like_atom(
                    preview.required("CHECK LIKE ends before all atoms are encoded")?,
                )?;
            }
            let mut atoms = Vec::new();
            budget.reserve_exact(&mut atoms, atom_count, "reserving decoded CHECK LIKE atoms")?;
            for _ in 0..atom_count {
                atoms.push(decode_like_atom(
                    fields.required("validated CHECK LIKE atoms remain available")?,
                )?);
            }
            Ok((
                CheckProgramNode::Predicate(CheckPredicate::Like { column, atoms }),
                None,
            ))
        }
        "ISNULL" | "NOTNULL" => {
            let (column, _) = decode_column(schema, fields)?;
            let predicate = if opcode.text == "ISNULL" {
                CheckPredicate::IsNull { column }
            } else {
                CheckPredicate::IsNotNull { column }
            };
            Ok((CheckProgramNode::Predicate(predicate), None))
        }
        "IN" => {
            let (column, definition) = decode_column(schema, fields)?;
            let count = fields.required("CHECK IN is missing its item count")?;
            let item_count = decode_usize(count, "invalid CHECK IN item count")?;
            if item_count == 0 {
                return Err(corrupt(count.offset, "CHECK IN requires at least one item"));
            }
            let mut preview = fields.clone();
            for _ in 0..item_count {
                let item = preview.required("CHECK IN ends before all items are encoded")?;
                validate_check_value_at(item.text, definition.data_type, item.offset)?;
            }
            let mut values = Vec::new();
            budget.reserve_exact(&mut values, item_count, "reserving decoded CHECK IN items")?;
            for _ in 0..item_count {
                let item = fields.required("validated CHECK IN items remain available")?;
                charge_text_operand(definition.data_type, item.text, item.offset, budget)?;
                values.push(decode_check_value_at(
                    item.text,
                    definition.data_type,
                    item.offset,
                )?);
            }
            Ok((
                CheckProgramNode::Predicate(CheckPredicate::In { column, values }),
                None,
            ))
        }
        _ => Err(corrupt(opcode.offset, "unknown CHECK program opcode")),
    }
}

pub(super) fn validate_node(
    schema: &TableSchema,
    opcode: Field<'_>,
    fields: &mut Fields<'_>,
) -> Result<ValidatedNode> {
    match opcode.text {
        "AND" | "OR" => {
            let count = fields.required("CHECK logical node is missing its child count")?;
            let children = decode_usize(count, "invalid CHECK child count")?;
            let operator = if opcode.text == "AND" {
                LogicalOperator::And
            } else {
                LogicalOperator::Or
            };
            Ok(ValidatedNode {
                logical: Some((operator, children, count.offset)),
                predicate_units: 0,
            })
        }
        "EQ" | "NE" | "LT" | "LE" | "GT" | "GE" => {
            let (_, definition) = decode_column(schema, fields)?;
            let operand = fields.required("CHECK comparison is missing its operand")?;
            if operand.text == "N" {
                return Err(corrupt(
                    operand.offset,
                    "CHECK comparison operands cannot be NULL",
                ));
            }
            validate_check_value_at(operand.text, definition.data_type, operand.offset)?;
            Ok(ValidatedNode {
                logical: None,
                predicate_units: 1,
            })
        }
        "LIKE" => {
            let (_, definition) = decode_column(schema, fields)?;
            if definition.data_type != DataType::Text {
                return Err(corrupt(opcode.offset, "CHECK LIKE requires a TEXT column"));
            }
            let count = fields.required("CHECK LIKE is missing its atom count")?;
            let atom_count = decode_usize(count, "invalid CHECK LIKE atom count")?;
            for _ in 0..atom_count {
                decode_like_atom(fields.required("CHECK LIKE ends before all atoms are encoded")?)?;
            }
            Ok(ValidatedNode {
                logical: None,
                predicate_units: 1,
            })
        }
        "ISNULL" | "NOTNULL" => {
            decode_column(schema, fields)?;
            Ok(ValidatedNode {
                logical: None,
                predicate_units: 1,
            })
        }
        "IN" => {
            let (_, definition) = decode_column(schema, fields)?;
            let count = fields.required("CHECK IN is missing its item count")?;
            let item_count = decode_usize(count, "invalid CHECK IN item count")?;
            if item_count == 0 {
                return Err(corrupt(count.offset, "CHECK IN requires at least one item"));
            }
            for _ in 0..item_count {
                let item = fields.required("CHECK IN ends before all items are encoded")?;
                validate_check_value_at(item.text, definition.data_type, item.offset)?;
            }
            Ok(ValidatedNode {
                logical: None,
                predicate_units: item_count,
            })
        }
        _ => Err(corrupt(opcode.offset, "unknown CHECK program opcode")),
    }
}

fn decode_column<'a>(
    schema: &'a TableSchema,
    fields: &mut Fields<'_>,
) -> Result<(usize, &'a crate::SchemaColumn)> {
    let field = fields.required("CHECK predicate is missing its column position")?;
    let column = decode_usize(field, "invalid CHECK column position")?;
    let definition = schema
        .columns
        .get(column)
        .ok_or_else(|| corrupt(field.offset, "CHECK column position is outside its table"))?;
    Ok((column, definition))
}

fn decode_like_atom(field: Field<'_>) -> Result<LikeAtom> {
    match field.text {
        "M" => Ok(LikeAtom::AnySequence),
        "S" => Ok(LikeAtom::AnyScalar),
        _ => {
            let payload = field
                .text
                .strip_prefix('L')
                .ok_or_else(|| corrupt(field.offset, "invalid CHECK LIKE atom"))?;
            if payload.is_empty() {
                return Err(corrupt(field.offset + 1, "empty CHECK LIKE literal atom"));
            }
            let mut literal = None;
            let mut count = 0_usize;
            let mut second_offset = None;
            scan_text(payload, field.offset + 1, |character, offset| {
                count += 1;
                if count == 2 {
                    second_offset = Some(offset);
                    return false;
                }
                literal = Some(character);
                true
            })?;
            if count != 1 {
                return Err(corrupt(
                    second_offset.unwrap_or(field.offset + 1),
                    "CHECK LIKE literal atom must encode one Unicode scalar",
                ));
            }
            Ok(LikeAtom::Literal(
                literal.expect("one decoded scalar was counted"),
            ))
        }
    }
}

fn charge_text_operand(
    data_type: DataType,
    encoded: &str,
    offset: usize,
    budget: &mut WorkingBudget,
) -> Result<()> {
    if data_type == DataType::Text && encoded != "N" {
        let payload = encoded
            .strip_prefix('T')
            .expect("validated CHECK TEXT operands have a canonical type prefix");
        let mut decoded_bytes = 0_usize;
        scan_text(payload, offset + 1, |character, _| {
            decoded_bytes += character.len_utf8();
            true
        })?;
        budget.charge(decoded_bytes)?;
    }
    Ok(())
}

pub(super) fn decode_usize(field: Field<'_>, message: &'static str) -> Result<usize> {
    let canonical = if field.text == "0" {
        true
    } else {
        let mut bytes = field.text.bytes();
        bytes
            .next()
            .is_some_and(|byte| (b'1'..=b'9').contains(&byte))
            && bytes.all(|byte| byte.is_ascii_digit())
    };
    if !canonical {
        return Err(corrupt(field.offset, message));
    }
    let value = field
        .text
        .parse::<u32>()
        .map_err(|_| corrupt(field.offset, message))?;
    usize::try_from(value).map_err(|_| corrupt(field.offset, message))
}

#[derive(Clone, Copy)]
pub(super) struct Field<'a> {
    pub(super) text: &'a str,
    pub(super) offset: usize,
}

#[derive(Clone)]
pub(super) struct Fields<'a> {
    remaining: Option<&'a str>,
    offset: usize,
    end_offset: usize,
}

impl<'a> Fields<'a> {
    pub(super) fn new(program: &'a str, offset: usize) -> Self {
        Self {
            remaining: Some(program),
            offset,
            end_offset: offset + program.len(),
        }
    }

    fn required(&mut self, message: &'static str) -> Result<Field<'a>> {
        self.next().ok_or_else(|| corrupt(self.end_offset, message))
    }

    pub(super) const fn end_offset(&self) -> usize {
        self.end_offset
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = Field<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.remaining?;
        let (text, next) = match remaining.split_once('|') {
            Some((field, rest)) => (field, Some(rest)),
            None => (remaining, None),
        };
        let field = Field {
            text,
            offset: self.offset,
        };
        self.offset += text.len() + usize::from(next.is_some());
        self.remaining = next;
        Some(field)
    }
}
