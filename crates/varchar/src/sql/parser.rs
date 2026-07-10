//! Recursive-descent statement parser for Varchar's small SQL dialect.

use super::ast::{
    Assignment, ColumnDef, CreateTable, Delete, ForeignKeyReference, Insert, Predicate,
    PredicateOperator, Projection, Select, Statement, Update,
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
        let mut table_primary_keys = Vec::new();
        let mut table_foreign_keys = Vec::new();
        loop {
            if self.current_word() == Some("PRIMARY") && self.peek_word() == Some("KEY") {
                table_primary_keys.push(self.parse_table_primary_key()?);
            } else if self.current_word() == Some("FOREIGN") && self.peek_word() == Some("KEY") {
                table_foreign_keys.push(self.parse_table_foreign_key()?);
            } else {
                columns.push(self.parse_column_definition()?);
            }
            if self.consume(&TokenKind::Comma) {
                continue;
            }
            self.expect(TokenKind::RightParen, "expected `,` or `)`")?;
            break;
        }

        for name in table_primary_keys {
            let column = columns
                .iter_mut()
                .find(|column| column.name == name)
                .ok_or_else(|| {
                    Error::Schema(format!(
                        "PRIMARY KEY references unknown column {name:?} in table {table:?}"
                    ))
                })?;
            if column.primary_key {
                return Err(Error::Schema(format!(
                    "duplicate PRIMARY KEY declaration for column {name:?}"
                )));
            }
            column.primary_key = true;
            column.nullable = false;
        }

        for (name, reference) in table_foreign_keys {
            let column = columns
                .iter_mut()
                .find(|column| column.name == name)
                .ok_or_else(|| {
                    Error::Schema(format!(
                        "FOREIGN KEY references unknown column {name:?} in table {table:?}"
                    ))
                })?;
            if column.references.is_some() {
                return Err(Error::Schema(format!(
                    "duplicate FOREIGN KEY declaration for column {name:?}"
                )));
            }
            column.references = Some(reference);
        }

        let primary_key_count = columns.iter().filter(|column| column.primary_key).count();
        if primary_key_count > 1 {
            return Err(Error::Schema(format!(
                "table {table:?} may have only one PRIMARY KEY column"
            )));
        }

        Ok(CreateTable { table, columns })
    }

    fn parse_column_definition(&mut self) -> Result<ColumnDef> {
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

        let mut nullable = true;
        let mut primary_key = false;
        let mut references = None;
        let mut saw_not_null = false;
        loop {
            match self.current_word() {
                Some("NOT") => {
                    if saw_not_null {
                        return Err(Error::Schema(format!(
                            "duplicate NOT NULL declaration for column {name:?}"
                        )));
                    }
                    self.advance();
                    self.expect_keyword("NULL")?;
                    nullable = false;
                    saw_not_null = true;
                }
                Some("PRIMARY") if self.peek_word() == Some("KEY") => {
                    if primary_key {
                        return Err(Error::Schema(format!(
                            "duplicate PRIMARY KEY declaration for column {name:?}"
                        )));
                    }
                    self.advance();
                    self.advance();
                    primary_key = true;
                    nullable = false;
                }
                Some("REFERENCES") => {
                    if references.is_some() {
                        return Err(Error::Schema(format!(
                            "duplicate REFERENCES declaration for column {name:?}"
                        )));
                    }
                    references = Some(self.parse_reference()?);
                }
                _ => break,
            }
        }

        Ok(ColumnDef {
            name,
            data_type,
            nullable,
            primary_key,
            references,
        })
    }

    fn parse_table_primary_key(&mut self) -> Result<String> {
        self.expect_keyword("PRIMARY")?;
        self.expect_keyword("KEY")?;
        self.expect(TokenKind::LeftParen, "expected `(` after PRIMARY KEY")?;
        let column = self.expect_identifier()?;
        self.reject_composite_constraint("PRIMARY KEY")?;
        self.expect(
            TokenKind::RightParen,
            "expected `)` after PRIMARY KEY column",
        )?;
        Ok(column)
    }

    fn parse_table_foreign_key(&mut self) -> Result<(String, ForeignKeyReference)> {
        self.expect_keyword("FOREIGN")?;
        self.expect_keyword("KEY")?;
        self.expect(TokenKind::LeftParen, "expected `(` after FOREIGN KEY")?;
        let column = self.expect_identifier()?;
        self.reject_composite_constraint("FOREIGN KEY")?;
        self.expect(
            TokenKind::RightParen,
            "expected `)` after FOREIGN KEY column",
        )?;
        let reference = self.parse_reference()?;
        Ok((column, reference))
    }

    fn parse_reference(&mut self) -> Result<ForeignKeyReference> {
        self.expect_keyword("REFERENCES")?;
        let table = self.expect_identifier()?;
        self.expect(TokenKind::LeftParen, "expected `(` after referenced table")?;
        let column = self.expect_identifier()?;
        self.reject_composite_constraint("FOREIGN KEY")?;
        self.expect(
            TokenKind::RightParen,
            "expected `)` after referenced column",
        )?;
        Ok(ForeignKeyReference { table, column })
    }

    fn reject_composite_constraint(&self, constraint: &str) -> Result<()> {
        if matches!(self.current().kind, TokenKind::Comma) {
            Err(Error::unsupported(
                format!("composite {constraint} constraints"),
                self.current().span,
            ))
        } else {
            Ok(())
        }
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
    use crate::sql::ast::{
        ColumnDef, CreateTable, ForeignKeyReference, Predicate, PredicateOperator, Projection,
        Select, Statement,
    };
    use crate::{DataType, Error, Value};

    fn create_table(sql: &str) -> CreateTable {
        match parse(sql).expect("CREATE TABLE parses") {
            Statement::CreateTable(statement) => statement,
            other => panic!("expected CREATE TABLE, got {other:?}"),
        }
    }

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

    #[test]
    fn parses_inline_primary_and_foreign_keys_in_either_modifier_order() {
        let statement = create_table(
            "CREATE TABLE children (\
                id INTEGER REFERENCES parents(id) PRIMARY KEY, \
                owner_id INTEGER NOT NULL REFERENCES owners(id), \
                note TEXT\
            )",
        );

        assert_eq!(
            statement.columns,
            vec![
                ColumnDef {
                    name: "id".to_owned(),
                    data_type: DataType::Integer,
                    nullable: false,
                    primary_key: true,
                    references: Some(ForeignKeyReference {
                        table: "parents".to_owned(),
                        column: "id".to_owned(),
                    }),
                },
                ColumnDef {
                    name: "owner_id".to_owned(),
                    data_type: DataType::Integer,
                    nullable: false,
                    primary_key: false,
                    references: Some(ForeignKeyReference {
                        table: "owners".to_owned(),
                        column: "id".to_owned(),
                    }),
                },
                ColumnDef {
                    name: "note".to_owned(),
                    data_type: DataType::Text,
                    nullable: true,
                    primary_key: false,
                    references: None,
                },
            ]
        );
    }

    #[test]
    fn normalizes_single_column_table_constraints_onto_columns() {
        let statement = create_table(
            "CREATE TABLE children (\
                id INTEGER, parent_id INTEGER, \
                PRIMARY KEY (id), \
                FOREIGN KEY (parent_id) REFERENCES parents(id)\
            )",
        );

        assert!(statement.columns[0].primary_key);
        assert!(!statement.columns[0].nullable);
        assert_eq!(
            statement.columns[1].references,
            Some(ForeignKeyReference {
                table: "parents".to_owned(),
                column: "id".to_owned(),
            })
        );
    }

    #[test]
    fn rejects_composite_key_constraints_explicitly() {
        for sql in [
            "CREATE TABLE t (a INTEGER, b INTEGER, PRIMARY KEY (a, b))",
            "CREATE TABLE t (a INTEGER, b INTEGER, FOREIGN KEY (a, b) REFERENCES p(a))",
            "CREATE TABLE t (a INTEGER REFERENCES p(a, b))",
        ] {
            assert!(
                matches!(parse(sql), Err(Error::Unsupported { .. })),
                "expected composite constraint to be unsupported: {sql}"
            );
        }
    }
}
