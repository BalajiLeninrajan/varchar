//! Handwritten parser for Varchar's deliberately small SQL dialect.

use crate::{DataType, Error, Result, Span, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Statement {
    CreateTable(CreateTable),
    Insert(Insert),
    Select(Select),
    Update(Update),
    Delete(Delete),
    ExplainRegex(Select),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CreateTable {
    pub(crate) table: String,
    pub(crate) columns: Vec<ColumnDef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ColumnDef {
    pub(crate) name: String,
    pub(crate) data_type: DataType,
    pub(crate) nullable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Insert {
    pub(crate) table: String,
    pub(crate) columns: Option<Vec<String>>,
    pub(crate) values: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Select {
    pub(crate) table: String,
    pub(crate) projection: Projection,
    pub(crate) predicates: Vec<Predicate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Projection {
    All,
    Columns(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Update {
    pub(crate) table: String,
    pub(crate) assignments: Vec<Assignment>,
    pub(crate) predicates: Vec<Predicate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Assignment {
    pub(crate) column: String,
    pub(crate) value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Delete {
    pub(crate) table: String,
    pub(crate) predicates: Vec<Predicate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Predicate {
    pub(crate) column: String,
    pub(crate) operator: PredicateOperator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PredicateOperator {
    Equal(Value),
    NotEqual(Value),
    Like(String),
    IsNull,
    IsNotNull,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TokenKind {
    Word(String),
    String(String),
    Number(String),
    LeftParen,
    RightParen,
    Comma,
    Star,
    Equal,
    NotEqual,
    Semicolon,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Token {
    kind: TokenKind,
    span: Span,
}

pub(crate) fn parse(input: &str) -> Result<Statement> {
    let tokens = lex(input)?;
    Parser::new(tokens).parse()
}

fn lex(input: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut cursor = 0;
    let bytes = input.as_bytes();

    while cursor < bytes.len() {
        let character = input[cursor..]
            .chars()
            .next()
            .expect("cursor is inside the input");
        let width = character.len_utf8();

        if character.is_whitespace() {
            cursor += width;
            continue;
        }

        let start = cursor;
        let kind = match character {
            '(' => {
                cursor += 1;
                TokenKind::LeftParen
            }
            ')' => {
                cursor += 1;
                TokenKind::RightParen
            }
            ',' => {
                cursor += 1;
                TokenKind::Comma
            }
            '*' => {
                cursor += 1;
                TokenKind::Star
            }
            '=' => {
                cursor += 1;
                TokenKind::Equal
            }
            ';' => {
                cursor += 1;
                TokenKind::Semicolon
            }
            '!' if bytes.get(cursor + 1) == Some(&b'=') => {
                cursor += 2;
                TokenKind::NotEqual
            }
            '!' => {
                return Err(Error::parse(
                    "expected `=` after `!`",
                    Span::new(start, start + 1),
                ));
            }
            '\'' => {
                cursor += 1;
                let mut value = String::new();
                let mut closed = false;
                while cursor < bytes.len() {
                    let next = input[cursor..]
                        .chars()
                        .next()
                        .expect("cursor is inside the input");
                    if next == '\'' {
                        if bytes.get(cursor + 1) == Some(&b'\'') {
                            value.push('\'');
                            cursor += 2;
                        } else {
                            cursor += 1;
                            closed = true;
                            break;
                        }
                    } else {
                        value.push(next);
                        cursor += next.len_utf8();
                    }
                }
                if !closed {
                    return Err(Error::parse(
                        "unterminated string literal",
                        Span::new(start, bytes.len()),
                    ));
                }
                TokenKind::String(value)
            }
            '"' => {
                return Err(Error::unsupported(
                    "quoted identifiers",
                    Span::new(start, start + 1),
                ));
            }
            '-' if bytes.get(cursor + 1) == Some(&b'-') => {
                return Err(Error::unsupported(
                    "SQL comments",
                    Span::new(start, bytes.len()),
                ));
            }
            '/' if bytes.get(cursor + 1) == Some(&b'*') => {
                return Err(Error::unsupported(
                    "SQL comments",
                    Span::new(start, bytes.len()),
                ));
            }
            '-' if bytes.get(cursor + 1).is_some_and(u8::is_ascii_digit) => {
                cursor += 1;
                while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                    cursor += 1;
                }
                TokenKind::Number(input[start..cursor].to_owned())
            }
            value if value.is_ascii_digit() => {
                cursor += 1;
                while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                    cursor += 1;
                }
                TokenKind::Number(input[start..cursor].to_owned())
            }
            value if value == '_' || value.is_ascii_alphabetic() => {
                cursor += 1;
                while bytes
                    .get(cursor)
                    .is_some_and(|byte| *byte == b'_' || byte.is_ascii_alphanumeric())
                {
                    cursor += 1;
                }
                TokenKind::Word(input[start..cursor].to_ascii_uppercase())
            }
            '<' | '>' => {
                return Err(Error::unsupported(
                    "ordered comparisons",
                    Span::new(start, start + width),
                ));
            }
            _ => {
                return Err(Error::parse(
                    format!("unexpected character {character:?}"),
                    Span::new(start, start + width),
                ));
            }
        };
        tokens.push(Token {
            kind,
            span: Span::new(start, cursor),
        });
    }

    tokens.push(Token {
        kind: TokenKind::End,
        span: Span::new(input.len(), input.len()),
    });
    Ok(tokens)
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
