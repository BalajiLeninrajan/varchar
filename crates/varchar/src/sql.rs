//! SQL syntax tree, tokenizer, and parser for Varchar's deliberately small dialect.

mod ast;
mod lexer;
mod parser;

use crate::Result;

pub(crate) use ast::{
    Assignment, ColumnModifier, ColumnRef, CreateElement, CreateTable, Delete, DescribeTable,
    Expression, ExpressionNode, ForeignKeyDeleteAction, ForeignKeyUpdateAction, Insert,
    OrderDirection, OrderTerm, Predicate, PredicateOperator, Projection, ProjectionItem, Select,
    Statement, TableConstraint, Update,
};

pub(crate) fn parse(input: &str) -> Result<Statement> {
    parser::parse(input)
}
