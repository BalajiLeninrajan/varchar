//! SQL syntax tree, tokenizer, and parser for Varchar's deliberately small dialect.

mod ast;
mod lexer;
mod parser;

use crate::Result;

#[cfg(test)]
pub(crate) use ast::Predicate;
pub(crate) use ast::{
    Assignment, ColumnModifier, ColumnRef, CreateElement, CreateTable, Delete, Expression,
    ExpressionNode, Insert, PredicateOperator, Projection, ProjectionItem, Select, Statement,
    TableConstraint, Update,
};

pub(crate) fn parse(input: &str) -> Result<Statement> {
    parser::parse(input)
}
