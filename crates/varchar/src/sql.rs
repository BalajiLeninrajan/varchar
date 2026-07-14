//! SQL syntax tree, tokenizer, and parser for Varchar's deliberately small dialect.

mod ast;
mod lexer;
mod parser;

use crate::Result;

pub(crate) use ast::{
    Assignment, CreateTable, Delete, Insert, Predicate, PredicateOperator, Projection, Select,
    Statement, Update,
};

pub(crate) fn parse(input: &str) -> Result<Statement> {
    parser::parse(input)
}
