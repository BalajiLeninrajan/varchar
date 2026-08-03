//! Recursive-descent statement parser for Varchar's small SQL dialect.

mod create;
mod expression;
mod mutation;
mod select;

use super::ast::{ColumnRef, Statement};
use super::lexer::{Token, TokenKind, lex};
use crate::{Error, Result, Value};

pub(super) fn parse(input: &str) -> Result<Statement> {
    let tokens = lex(input)?;
    Parser::new(tokens).parse()
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            position: 0,
        }
    }

    fn parse(mut self) -> Result<Statement> {
        let statement = match self.current_word() {
            Some("CREATE") => Statement::CreateTable(self.parse_create_table()?),
            Some("INSERT") => Statement::Insert(self.parse_insert()?),
            Some("SELECT") => Statement::Select(self.parse_select()?),
            Some("UPDATE") => Statement::Update(self.parse_update()?),
            Some("DELETE") => Statement::Delete(self.parse_delete()?),
            Some("EXPLAIN") => Statement::ExplainRegex(self.parse_explain()?),
            Some(keyword) => {
                return Err(Error::unsupported(
                    format!("statement `{keyword}`"),
                    self.current().span,
                ));
            }
            None => {
                return Err(Error::parse(
                    "expected a SQL statement",
                    self.current().span,
                ));
            }
        };

        if self.consume(&TokenKind::Semicolon) && !self.at_end() {
            return Err(Error::unsupported(
                "multiple SQL statements",
                self.current().span,
            ));
        }
        if !self.at_end() {
            let feature = self
                .current_word()
                .and_then(trailing_feature)
                .unwrap_or("trailing SQL syntax");
            return Err(Error::unsupported(feature, self.current().span));
        }
        Ok(statement)
    }

    fn parse_column_ref(&mut self) -> Result<ColumnRef> {
        let first = self.expect_identifier()?;
        if self.consume(&TokenKind::Dot) {
            Ok(ColumnRef {
                qualifier: Some(first),
                name: self.expect_identifier()?,
            })
        } else {
            Ok(ColumnRef {
                qualifier: None,
                name: first,
            })
        }
    }

    fn parse_identifier_list(&mut self) -> Result<Vec<String>> {
        let mut names = vec![self.expect_identifier()?];
        while self.consume(&TokenKind::Comma) {
            names.push(self.expect_identifier()?);
        }
        Ok(names)
    }

    fn parse_value_list(&mut self) -> Result<Vec<Value>> {
        let mut values = vec![self.parse_value()?];
        while self.consume(&TokenKind::Comma) {
            values.push(self.parse_value()?);
        }
        Ok(values)
    }

    fn parse_value(&mut self) -> Result<Value> {
        let span = self.current().span;
        let value = match self.current().kind.clone() {
            TokenKind::String(value) => Value::Text(value),
            TokenKind::Number(value) => Value::Integer(
                value
                    .parse()
                    .map_err(|_| Error::parse("INTEGER literal is outside the i64 range", span))?,
            ),
            TokenKind::Word(word) if word == "TRUE" => Value::Boolean(true),
            TokenKind::Word(word) if word == "FALSE" => Value::Boolean(false),
            TokenKind::Word(word) if word == "NULL" => Value::Null,
            _ => return Err(Error::parse("expected a literal value", span)),
        };
        self.advance();
        Ok(value)
    }

    fn expect_identifier(&mut self) -> Result<String> {
        let span = self.current().span;
        match self.current().kind.clone() {
            TokenKind::Word(word) if !is_reserved(&word) => {
                self.advance();
                Ok(word.to_ascii_lowercase())
            }
            TokenKind::Word(word) => Err(Error::parse(
                format!("reserved keyword `{word}` cannot be used as an identifier"),
                span,
            )),
            _ => Err(Error::parse("expected an unquoted identifier", span)),
        }
    }

    fn expect_keyword(&mut self, expected: &str) -> Result<()> {
        if self.consume_keyword(expected) {
            Ok(())
        } else {
            Err(Error::parse(
                format!("expected keyword {expected}"),
                self.current().span,
            ))
        }
    }

    fn expect(&mut self, expected: TokenKind, message: &str) -> Result<()> {
        if self.consume(&expected) {
            Ok(())
        } else {
            Err(Error::parse(message, self.current().span))
        }
    }

    fn consume_keyword(&mut self, expected: &str) -> bool {
        if self.current_word() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn consume(&mut self, expected: &TokenKind) -> bool {
        if &self.current().kind == expected {
            self.advance();
            true
        } else {
            false
        }
    }

    fn current_word(&self) -> Option<&str> {
        match &self.current().kind {
            TokenKind::Word(word) => Some(word),
            _ => None,
        }
    }

    fn peek_word(&self) -> Option<&str> {
        match self.tokens.get(self.position + 1).map(|token| &token.kind) {
            Some(TokenKind::Word(word)) => Some(word),
            _ => None,
        }
    }

    fn current(&self) -> &Token {
        &self.tokens[self.position]
    }

    fn advance(&mut self) {
        if !self.at_end() {
            self.position += 1;
        }
    }

    fn at_end(&self) -> bool {
        matches!(self.current().kind, TokenKind::End)
    }
}

fn trailing_feature(word: &str) -> Option<&'static str> {
    match word {
        "OR" => Some("OR predicates"),
        "JOIN" => Some("joins"),
        "LEFT" | "RIGHT" | "FULL" | "OUTER" => Some("outer joins"),
        "CROSS" => Some("cross joins"),
        "NATURAL" => Some("natural joins"),
        "ORDER" => Some("ORDER BY"),
        "GROUP" => Some("GROUP BY"),
        "LIMIT" => Some("LIMIT"),
        "AS" => Some("aliases"),
        _ => None,
    }
}

fn is_reserved(word: &str) -> bool {
    matches!(
        word,
        "CREATE"
            | "TABLE"
            | "INSERT"
            | "INTO"
            | "VALUES"
            | "SELECT"
            | "FROM"
            | "WHERE"
            | "UPDATE"
            | "SET"
            | "DELETE"
            | "EXPLAIN"
            | "REGEX"
            | "AND"
            | "OR"
            | "IS"
            | "NOT"
            | "NULL"
            | "LIKE"
            | "TEXT"
            | "INTEGER"
            | "BOOLEAN"
            | "TRUE"
            | "FALSE"
            | "AS"
            | "JOIN"
            | "ORDER"
            | "BY"
            | "GROUP"
            | "LIMIT"
            | "ALTER"
            | "DROP"
            | "DEFAULT"
    )
}

#[cfg(test)]
mod tests;
