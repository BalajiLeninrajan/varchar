use crate::Result;
use crate::expression::{CheckPredicate, CheckProgramNode};
use crate::storage::TableSchema;
use crate::storage::format::corrupt;

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
        "ISNULL" | "NOTNULL" => {
            let (column, _) = decode_column(schema, fields)?;
            let predicate = if opcode.text == "ISNULL" {
                CheckPredicate::IsNull { column }
            } else {
                CheckPredicate::IsNotNull { column }
            };
            Ok((CheckProgramNode::Predicate(predicate), None))
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
        "ISNULL" | "NOTNULL" => {
            decode_column(schema, fields)?;
            Ok(ValidatedNode {
                logical: None,
                predicate_units: 1,
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
