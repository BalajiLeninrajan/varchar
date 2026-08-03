//! SQL syntax tree, tokenizer, and parser for Varchar's deliberately small dialect.

mod ast;
mod lexer;
mod parser;

use crate::Result;

pub(crate) use ast::{
    Assignment, ColumnModifier, ColumnRef, CreateElement, CreateTable, Delete, DescribeTable,
    Expression, ExpressionNode, ForeignKeyDeleteAction, ForeignKeyUpdateAction, Insert,
    OrderDirection, OrderTerm, Predicate, PredicateOperator, Projection, ProjectionItem, Select,
    ShowCreateTable, Statement, TableConstraint, Update,
};

pub(crate) fn parse(input: &str) -> Result<Statement> {
    parser::parse(input)
}

pub(crate) fn is_reserved_identifier(word: &str) -> bool {
    const RESERVED: &[&str] = &[
        "CREATE", "TABLE", "INSERT", "INTO", "VALUES", "SELECT", "FROM", "WHERE", "UPDATE", "SET",
        "DELETE", "EXPLAIN", "REGEX", "AND", "OR", "IS", "IN", "NOT", "NULL", "LIKE", "TEXT",
        "INTEGER", "BOOLEAN", "TRUE", "FALSE", "AS", "JOIN", "ORDER", "BY", "ASC", "DESC", "GROUP",
        "LIMIT", "OFFSET", "ALTER", "DROP", "DEFAULT", "UNIQUE", "CHECK", "CASCADE", "RESTRICT",
        "SHOW", "DESCRIBE", "TABLES",
    ];

    RESERVED
        .iter()
        .any(|keyword| word.eq_ignore_ascii_case(keyword))
}
