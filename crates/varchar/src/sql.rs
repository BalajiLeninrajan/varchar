//! SQL syntax tree, tokenizer, and parser for Varchar's deliberately small dialect.

mod ast;
mod lexer;
mod parser;

use crate::Result;

pub(crate) use ast::{
    Assignment, ColumnModifier, CreateElement, CreateTable, Delete, Insert, Predicate,
    PredicateOperator, Projection, Select, Statement, TableConstraint, Update,
};

pub(crate) fn parse(input: &str) -> Result<Statement> {
    parser::parse(input)
}
