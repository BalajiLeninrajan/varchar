//! Recursive-descent statement parser for Varchar's small SQL dialect.

use super::ast::{
    Assignment, ColumnDef, CreateTable, Delete, Insert, Predicate, PredicateOperator, Projection,
    Select, Statement, Update,
};
use super::lexer::{Token, TokenKind, lex};
use crate::{DataType, Error, Result, Value};

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
            let feature = match self.current_word() {
                Some("OR") => "OR predicates",
                Some("JOIN") => "joins",
                Some("ORDER") => "ORDER BY",
                Some("GROUP") => "GROUP BY",
                Some("LIMIT") => "LIMIT",
                Some("AS") => "aliases",
                _ => "trailing SQL syntax",
            };
            return Err(Error::unsupported(feature, self.current().span));
        }
        Ok(statement)
    }

    fn parse_create_table(&mut self) -> Result<CreateTable> {
        self.expect_keyword("CREATE")?;
        self.expect_keyword("TABLE")?;
        let table = self.expect_identifier()?;
        self.expect(TokenKind::LeftParen, "expected `(` after table name")?;
        let mut columns = Vec::new();
        loop {
            let name = self.expect_identifier()?;
            let data_type = match self.current_word() {
                Some("TEXT") => DataType::Text,
                Some("INTEGER") => DataType::Integer,
                Some("BOOLEAN") => DataType::Boolean,
                Some(other) => {
                    return Err(Error::unsupported(
                        format!("column type `{other}`"),
                        self.current().span,
                    ));
                }
                None => {
                    return Err(Error::parse(
                        "expected TEXT, INTEGER, or BOOLEAN",
                        self.current().span,
                    ));
                }
            };
            self.advance();
            let nullable = if self.consume_keyword("NOT") {
                self.expect_keyword("NULL")?;
                false
            } else {
                true
            };
            columns.push(ColumnDef {
                name,
                data_type,
                nullable,
            });
            if self.consume(&TokenKind::Comma) {
                continue;
            }
            self.expect(TokenKind::RightParen, "expected `,` or `)`")?;
            break;
        }
        Ok(CreateTable { table, columns })
    }

    fn parse_insert(&mut self) -> Result<Insert> {
        self.expect_keyword("INSERT")?;
        self.expect_keyword("INTO")?;
        let table = self.expect_identifier()?;
        let columns = if self.consume(&TokenKind::LeftParen) {
            let names = self.parse_identifier_list()?;
            self.expect(TokenKind::RightParen, "expected `)` after column list")?;
            Some(names)
        } else {
            None
        };
        self.expect_keyword("VALUES")?;
        self.expect(TokenKind::LeftParen, "expected `(` after VALUES")?;
        let values = self.parse_value_list()?;
        self.expect(TokenKind::RightParen, "expected `)` after VALUES")?;
        if matches!(self.current().kind, TokenKind::Comma) {
            return Err(Error::unsupported("multi-row INSERT", self.current().span));
        }
        Ok(Insert {
            table,
            columns,
            values,
        })
    }

    fn parse_select(&mut self) -> Result<Select> {
        self.expect_keyword("SELECT")?;
        let projection = if self.consume(&TokenKind::Star) {
            Projection::All
        } else {
            Projection::Columns(self.parse_identifier_list()?)
        };
        self.expect_keyword("FROM")?;
        let table = self.expect_identifier()?;
        let predicates = self.parse_optional_where()?;
        Ok(Select {
            table,
            projection,
            predicates,
        })
    }

    fn parse_update(&mut self) -> Result<Update> {
        self.expect_keyword("UPDATE")?;
        let table = self.expect_identifier()?;
        self.expect_keyword("SET")?;
        let mut assignments = Vec::new();
        loop {
            let column = self.expect_identifier()?;
            self.expect(TokenKind::Equal, "expected `=` in assignment")?;
            let value = self.parse_value()?;
            assignments.push(Assignment { column, value });
            if !self.consume(&TokenKind::Comma) {
                break;
            }
        }
        let predicates = self.parse_optional_where()?;
        Ok(Update {
            table,
            assignments,
            predicates,
        })
    }

    fn parse_delete(&mut self) -> Result<Delete> {
        self.expect_keyword("DELETE")?;
        self.expect_keyword("FROM")?;
        let table = self.expect_identifier()?;
        let predicates = self.parse_optional_where()?;
        Ok(Delete { table, predicates })
    }

    fn parse_explain(&mut self) -> Result<Select> {
        self.expect_keyword("EXPLAIN")?;
        self.expect_keyword("REGEX")?;
        if self.current_word() != Some("SELECT") {
            return Err(Error::unsupported(
                "EXPLAIN REGEX only supports SELECT",
                self.current().span,
            ));
        }
        self.parse_select()
    }

    fn parse_optional_where(&mut self) -> Result<Vec<Predicate>> {
        if !self.consume_keyword("WHERE") {
            return Ok(Vec::new());
        }
        let mut predicates = vec![self.parse_predicate()?];
        while self.consume_keyword("AND") {
            predicates.push(self.parse_predicate()?);
        }
        if self.current_word() == Some("OR") {
            return Err(Error::unsupported("OR predicates", self.current().span));
        }
        Ok(predicates)
    }

    fn parse_predicate(&mut self) -> Result<Predicate> {
        let column = self.expect_identifier()?;
        let operator = match self.current().kind.clone() {
            TokenKind::Equal => {
                self.advance();
                PredicateOperator::Equal(self.parse_value()?)
            }
            TokenKind::NotEqual => {
                self.advance();
                PredicateOperator::NotEqual(self.parse_value()?)
            }
            TokenKind::Word(ref word) if word == "LIKE" => {
                self.advance();
                match self.current().kind.clone() {
                    TokenKind::String(pattern) => {
                        self.advance();
                        PredicateOperator::Like(pattern)
                    }
                    _ => {
                        return Err(Error::parse(
                            "LIKE expects a string literal",
                            self.current().span,
                        ));
                    }
                }
            }
            TokenKind::Word(ref word) if word == "IS" => {
                self.advance();
                let negated = self.consume_keyword("NOT");
                self.expect_keyword("NULL")?;
                if negated {
                    PredicateOperator::IsNotNull
                } else {
                    PredicateOperator::IsNull
                }
            }
            _ => {
                return Err(Error::parse(
                    "expected `=`, `!=`, `LIKE`, or `IS [NOT] NULL`",
                    self.current().span,
                ));
            }
        };
        Ok(Predicate { column, operator })
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
mod tests {
    use super::parse;
    use crate::sql::ast::{Predicate, PredicateOperator, Projection, Select, Statement};
    use crate::{Error, Value};

    #[test]
    fn parsing_produces_the_exact_normalized_ast() {
        assert_eq!(
            parse("SeLeCt Name, ID FROM Users WHERE Name LIKE 'a_%' AND ID != -7;")
                .expect("SELECT parses"),
            Statement::Select(Select {
                table: String::from("users"),
                projection: Projection::Columns(vec![String::from("name"), String::from("id"),]),
                predicates: vec![
                    Predicate {
                        column: String::from("name"),
                        operator: PredicateOperator::Like(String::from("a_%")),
                    },
                    Predicate {
                        column: String::from("id"),
                        operator: PredicateOperator::NotEqual(Value::Integer(-7)),
                    },
                ],
            })
        );
    }

    #[test]
    fn unsupported_trailing_syntax_keeps_its_feature_and_span() {
        assert!(matches!(
            parse("SELECT * FROM t JOIN u"),
            Err(Error::Unsupported {
                ref feature,
                span_start: 16,
                span_end: 20,
            }) if feature == "joins"
        ));
    }
}
