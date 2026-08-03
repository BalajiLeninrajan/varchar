//! Recursive-descent statement parser for Varchar's small SQL dialect.

mod create;
mod expression;
mod metadata;
mod mutation;
mod pagination;
mod select;

use std::ops::Range;

use super::ast::{ColumnRef, Statement};
use super::lexer::{
    Token, TokenKind, comparison_error, lex_for_parser, unexpected_character_error,
};
use crate::{Error, Result, Span, Value};

pub(super) fn parse(input: &str) -> Result<Statement> {
    let tokens = lex_for_parser(input)?;
    let mut parser = Parser::new(tokens);
    let result = parser.parse();
    parser.reject_deferred_lexical_errors()?;
    result
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
    where_expression: Option<Range<usize>>,
    check_expressions: Vec<Range<usize>>,
    claimed_in_expression: Option<usize>,
    claimed_order_error: Option<usize>,
    claimed_pagination_error: Option<usize>,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            position: 0,
            where_expression: None,
            check_expressions: Vec::new(),
            claimed_in_expression: None,
            claimed_order_error: None,
            claimed_pagination_error: None,
        }
    }

    fn parse(&mut self) -> Result<Statement> {
        let statement = match self.current_word() {
            Some("CREATE") => Statement::CreateTable(self.parse_create_table()?),
            Some("INSERT") => Statement::Insert(self.parse_insert()?),
            Some("SELECT") => Statement::Select(self.parse_select()?),
            Some("UPDATE") => Statement::Update(self.parse_update()?),
            Some("DELETE") => Statement::Delete(self.parse_delete()?),
            Some("SHOW") => self.parse_show()?,
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

    fn reject_deferred_lexical_errors(&self) -> Result<()> {
        let mut index = 0;
        while let Some(token) = self.tokens.get(index) {
            if self.claimed_order_error == Some(index)
                || self.claimed_pagination_error == Some(index)
            {
                break;
            }
            if let TokenKind::LexicalError(error) = &token.kind {
                return Err(error.error(token.span));
            }

            if comparison_fragment(&token.kind).is_some() {
                if self.token_is_in_expression(index) {
                    let (operator, end, span) = self.comparison_sequence(index);
                    if !is_expression_comparison(&operator) {
                        return Err(comparison_error(&operator, span));
                    }
                    if self.claimed_in_expression == Some(index) {
                        break;
                    }
                    index = end;
                    continue;
                }

                match &token.kind {
                    TokenKind::LessThan | TokenKind::GreaterThan => {
                        return Err(Error::unsupported(
                            "ordered comparisons",
                            Span::new(token.span.start, token.span.start + 1),
                        ));
                    }
                    TokenKind::Bang => return Err(comparison_error("!", token.span)),
                    _ => {}
                }
            }

            if self.claimed_in_expression == Some(index) {
                break;
            }
            if let TokenKind::ExpressionOperator(character) = &token.kind {
                return Err(unexpected_character_error(*character, token.span));
            }
            index += 1;
        }
        Ok(())
    }

    fn comparison_sequence(&self, start: usize) -> (String, usize, Span) {
        let mut operator = String::new();
        let mut end = start;
        let mut span_end = self.tokens[start].span.start;
        while let Some(token) = self.tokens.get(end) {
            if end > start && token.span.start != span_end {
                break;
            }
            let Some(fragment) = comparison_fragment(&token.kind) else {
                break;
            };
            operator.push_str(fragment);
            span_end = token.span.end;
            end += 1;
        }
        (
            operator,
            end,
            Span::new(self.tokens[start].span.start, span_end),
        )
    }

    pub(super) fn register_check_expression(&mut self, start: usize) -> Result<()> {
        let end = self.check_expression_end(start);
        self.check_expressions
            .try_reserve(1)
            .map_err(|_| Error::Allocation {
                operation: "recording CHECK expression tokens",
            })?;
        self.check_expressions.push(start..end);
        Ok(())
    }

    fn check_expression_end(&self, start: usize) -> usize {
        let mut depth = 0_usize;
        let mut index = start;
        while let Some(token) = self.tokens.get(index) {
            if matches!(&token.kind, TokenKind::End | TokenKind::Semicolon) {
                break;
            }
            match &token.kind {
                TokenKind::LeftParen => depth = depth.saturating_add(1),
                TokenKind::RightParen if depth > 0 => depth -= 1,
                _ => {}
            }
            index += 1;
            if depth == 0 {
                break;
            }
        }
        index
    }

    fn token_is_in_expression(&self, index: usize) -> bool {
        self.where_expression
            .as_ref()
            .is_some_and(|range| range.contains(&index))
            || self
                .check_expressions
                .iter()
                .any(|range| range.contains(&index))
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

    fn peek_is(&self, expected: &TokenKind) -> bool {
        self.tokens
            .get(self.position + 1)
            .is_some_and(|token| &token.kind == expected)
    }

    fn peek_is_adjacent(&self, expected: &TokenKind) -> bool {
        self.tokens.get(self.position + 1).is_some_and(|token| {
            &token.kind == expected && self.current().span.end == token.span.start
        })
    }

    fn current_word(&self) -> Option<&str> {
        self.word_at(self.position)
    }

    fn peek_word(&self) -> Option<&str> {
        self.word_at(self.position + 1)
    }

    fn word_at(&self, index: usize) -> Option<&str> {
        match self.tokens.get(index).map(|token| &token.kind) {
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

fn comparison_fragment(kind: &TokenKind) -> Option<&'static str> {
    match kind {
        TokenKind::Bang => Some("!"),
        TokenKind::Equal => Some("="),
        TokenKind::NotEqual => Some("!="),
        TokenKind::LessThan => Some("<"),
        TokenKind::GreaterThan => Some(">"),
        _ => None,
    }
}

fn is_expression_comparison(operator: &str) -> bool {
    matches!(operator, "=" | "!=" | "<" | "<=" | ">" | ">=")
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
        "OFFSET" => Some("OFFSET"),
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
            | "IN"
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
            | "ASC"
            | "DESC"
            | "GROUP"
            | "LIMIT"
            | "OFFSET"
            | "ALTER"
            | "DROP"
            | "DEFAULT"
            | "UNIQUE"
            | "CHECK"
            | "CASCADE"
            | "RESTRICT"
            | "SHOW"
            | "TABLES"
    )
}

#[cfg(test)]
mod tests;
