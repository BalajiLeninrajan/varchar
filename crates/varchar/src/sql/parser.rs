//! Recursive-descent statement parser for Varchar's small SQL dialect.

use super::ast::{
    Assignment, ColumnDef, ColumnModifier, ColumnRef, CreateElement, CreateTable, Delete,
    ForeignKeyReference, Insert, Join, JoinCondition, Predicate, PredicateOperator, Projection,
    ProjectionItem, Select, Statement, TableConstraint, Update,
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
                Some("LEFT" | "RIGHT" | "FULL" | "OUTER") => "outer joins",
                Some("CROSS") => "cross joins",
                Some("NATURAL") => "natural joins",
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
        let mut elements = Vec::new();
        loop {
            let element = if self.current_word() == Some("PRIMARY")
                && self.peek_word() == Some("KEY")
            {
                CreateElement::Constraint(self.parse_table_primary_key()?)
            } else if self.current_word() == Some("FOREIGN") && self.peek_word() == Some("KEY") {
                CreateElement::Constraint(self.parse_table_foreign_key()?)
            } else {
                CreateElement::Column(self.parse_column_definition()?)
            };
            elements.push(element);
            if self.consume(&TokenKind::Comma) {
                continue;
            }
            self.expect(TokenKind::RightParen, "expected `,` or `)`")?;
            break;
        }
        Ok(CreateTable { table, elements })
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

        let mut modifiers = Vec::new();
        loop {
            match self.current_word() {
                Some("NOT") => {
                    self.advance();
                    self.expect_keyword("NULL")?;
                    modifiers.push(ColumnModifier::NotNull);
                }
                Some("PRIMARY") if self.peek_word() == Some("KEY") => {
                    self.advance();
                    self.advance();
                    modifiers.push(ColumnModifier::PrimaryKey);
                }
                Some("REFERENCES") => {
                    modifiers.push(ColumnModifier::References(self.parse_reference()?));
                }
                Some("AUTOINCREMENT" | "AUTO_INCREMENT") => {
                    self.advance();
                    modifiers.push(ColumnModifier::AutoIncrement);
                }
                _ => break,
            }
        }

        Ok(ColumnDef {
            name,
            data_type,
            modifiers,
        })
    }

    fn parse_table_primary_key(&mut self) -> Result<TableConstraint> {
        self.expect_keyword("PRIMARY")?;
        self.expect_keyword("KEY")?;
        self.expect(TokenKind::LeftParen, "expected `(` after PRIMARY KEY")?;
        let column = self.expect_identifier()?;
        self.reject_composite_constraint("PRIMARY KEY")?;
        self.expect(
            TokenKind::RightParen,
            "expected `)` after PRIMARY KEY column",
        )?;
        Ok(TableConstraint::PrimaryKey(column))
    }

    fn parse_table_foreign_key(&mut self) -> Result<TableConstraint> {
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
        Ok(TableConstraint::ForeignKey { column, reference })
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
        let projection = self.parse_projection()?;
        self.expect_keyword("FROM")?;
        let table = self.expect_identifier()?;
        let joins = self.parse_joins(&table)?;
        let predicates = self.parse_optional_where()?;
        Ok(Select {
            table,
            joins,
            projection,
            predicates,
        })
    }

    fn parse_projection(&mut self) -> Result<Projection> {
        if self.consume(&TokenKind::Star) {
            return Ok(Projection::All);
        }

        let mut items = vec![self.parse_projection_item()?];
        while self.consume(&TokenKind::Comma) {
            items.push(self.parse_projection_item()?);
        }
        Ok(Projection::Items(items))
    }

    fn parse_projection_item(&mut self) -> Result<ProjectionItem> {
        let first = self.expect_identifier()?;
        if !self.consume(&TokenKind::Dot) {
            return Ok(ProjectionItem::Column(ColumnRef {
                qualifier: None,
                name: first,
            }));
        }

        if self.consume(&TokenKind::Star) {
            return Ok(ProjectionItem::QualifiedAll(first));
        }

        let name = self.expect_identifier()?;
        Ok(ProjectionItem::Column(ColumnRef {
            qualifier: Some(first),
            name,
        }))
    }

    fn parse_joins(&mut self, base_table: &str) -> Result<Vec<Join>> {
        let mut joins = Vec::new();
        loop {
            if self.current_word() == Some("INNER") && self.peek_word() == Some("JOIN") {
                self.advance();
                self.advance();
            } else if self.consume_keyword("JOIN") {
                // Bare JOIN is an INNER JOIN.
            } else {
                break;
            }

            let table_span = self.current().span;
            let table = self.expect_identifier()?;
            if table == base_table || joins.iter().any(|join: &Join| join.table == table) {
                return Err(Error::unsupported("self joins", table_span));
            }
            if self.current_word() == Some("AS") {
                return Err(Error::unsupported("aliases", self.current().span));
            }

            self.expect_keyword("ON")?;
            let mut conditions = vec![self.parse_join_condition()?];
            while self.consume_keyword("AND") {
                conditions.push(self.parse_join_condition()?);
            }
            joins.push(Join { table, conditions });
        }
        Ok(joins)
    }

    fn parse_join_condition(&mut self) -> Result<JoinCondition> {
        let left = self.parse_column_ref()?;
        self.expect(TokenKind::Equal, "expected `=` in JOIN condition")?;
        let right = self.parse_column_ref()?;
        Ok(JoinCondition { left, right })
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
        let column = self.parse_column_ref()?;
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
        ColumnDef, ColumnModifier, ColumnRef, CreateElement, CreateTable, ForeignKeyReference,
        Join, JoinCondition, Predicate, PredicateOperator, Projection, ProjectionItem, Select,
        Statement, TableConstraint,
    };
    use crate::{DataType, Error, Value};

    fn create_table(sql: &str) -> CreateTable {
        match parse(sql).expect("CREATE TABLE parses") {
            Statement::CreateTable(statement) => statement,
            other => panic!("expected CREATE TABLE, got {other:?}"),
        }
    }

    fn select(sql: &str) -> Select {
        match parse(sql).expect("SELECT parses") {
            Statement::Select(statement) => statement,
            other => panic!("expected SELECT, got {other:?}"),
        }
    }

    fn column_ref(qualifier: Option<&str>, name: &str) -> ColumnRef {
        ColumnRef {
            qualifier: qualifier.map(str::to_owned),
            name: name.to_owned(),
        }
    }

    #[test]
    fn parsing_produces_the_exact_normalized_ast() {
        assert_eq!(
            parse("SeLeCt Name, ID FROM Users WHERE Name LIKE 'a_%' AND ID != -7;")
                .expect("SELECT parses"),
            Statement::Select(Select {
                table: String::from("users"),
                joins: Vec::new(),
                projection: Projection::Items(vec![
                    ProjectionItem::Column(column_ref(None, "name")),
                    ProjectionItem::Column(column_ref(None, "id")),
                ]),
                predicates: vec![
                    Predicate {
                        column: column_ref(None, "name"),
                        operator: PredicateOperator::Like(String::from("a_%")),
                    },
                    Predicate {
                        column: column_ref(None, "id"),
                        operator: PredicateOperator::NotEqual(Value::Integer(-7)),
                    },
                ],
            })
        );
    }

    #[test]
    fn unsupported_join_syntax_keeps_its_feature_and_span() {
        assert!(matches!(
            parse("SELECT * FROM t LEFT JOIN u ON t.id = u.id"),
            Err(Error::Unsupported {
                ref feature,
                span_start: 16,
                span_end: 20,
            }) if feature == "outer joins"
        ));
    }

    #[test]
    fn parses_qualified_projection_inner_join_and_predicate_ast() {
        assert_eq!(
            select(
                "SELECT authors.name, books.* FROM authors INNER JOIN books \
                 ON authors.id = books.author_id AND authors.kind = books.kind \
                 WHERE books.title LIKE 'R%'",
            ),
            Select {
                table: "authors".to_owned(),
                joins: vec![Join {
                    table: "books".to_owned(),
                    conditions: vec![
                        JoinCondition {
                            left: column_ref(Some("authors"), "id"),
                            right: column_ref(Some("books"), "author_id"),
                        },
                        JoinCondition {
                            left: column_ref(Some("authors"), "kind"),
                            right: column_ref(Some("books"), "kind"),
                        },
                    ],
                }],
                projection: Projection::Items(vec![
                    ProjectionItem::Column(column_ref(Some("authors"), "name")),
                    ProjectionItem::QualifiedAll("books".to_owned()),
                ]),
                predicates: vec![Predicate {
                    column: column_ref(Some("books"), "title"),
                    operator: PredicateOperator::Like("R%".to_owned()),
                }],
            }
        );
    }

    #[test]
    fn inner_and_on_remain_contextual_identifiers() {
        let statement = create_table("CREATE TABLE inner (on INTEGER)");
        assert_eq!(statement.table, "inner");
        let CreateElement::Column(column) = &statement.elements[0] else {
            panic!("expected a column");
        };
        assert_eq!(column.name, "on");

        let statement = select("SELECT inner.on FROM inner WHERE inner.on = 1");
        assert_eq!(
            statement.projection,
            Projection::Items(vec![ProjectionItem::Column(column_ref(
                Some("inner"),
                "on",
            ))])
        );
        assert_eq!(
            statement.predicates[0].column,
            column_ref(Some("inner"), "on")
        );
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
            statement.elements,
            vec![
                CreateElement::Column(ColumnDef {
                    name: "id".to_owned(),
                    data_type: DataType::Integer,
                    modifiers: vec![
                        ColumnModifier::References(ForeignKeyReference {
                            table: "parents".to_owned(),
                            column: "id".to_owned(),
                        }),
                        ColumnModifier::PrimaryKey,
                    ],
                }),
                CreateElement::Column(ColumnDef {
                    name: "owner_id".to_owned(),
                    data_type: DataType::Integer,
                    modifiers: vec![
                        ColumnModifier::NotNull,
                        ColumnModifier::References(ForeignKeyReference {
                            table: "owners".to_owned(),
                            column: "id".to_owned(),
                        }),
                    ],
                }),
                CreateElement::Column(ColumnDef {
                    name: "note".to_owned(),
                    data_type: DataType::Text,
                    modifiers: Vec::new(),
                }),
            ]
        );
    }

    #[test]
    fn preserves_table_elements_in_source_order() {
        let statement = create_table(
            "CREATE TABLE children (\
                PRIMARY KEY (id), \
                id INTEGER, \
                FOREIGN KEY (parent_id) REFERENCES parents(id), \
                parent_id INTEGER\
            )",
        );

        assert_eq!(
            statement.elements,
            vec![
                CreateElement::Constraint(TableConstraint::PrimaryKey("id".to_owned())),
                CreateElement::Column(ColumnDef {
                    name: "id".to_owned(),
                    data_type: DataType::Integer,
                    modifiers: Vec::new(),
                }),
                CreateElement::Constraint(TableConstraint::ForeignKey {
                    column: "parent_id".to_owned(),
                    reference: ForeignKeyReference {
                        table: "parents".to_owned(),
                        column: "id".to_owned(),
                    },
                }),
                CreateElement::Column(ColumnDef {
                    name: "parent_id".to_owned(),
                    data_type: DataType::Integer,
                    modifiers: Vec::new(),
                }),
            ]
        );
    }

    #[test]
    fn preserves_duplicate_declarations_for_semantic_resolution() {
        let statement = create_table(
            "CREATE TABLE items (\
                id INTEGER NOT NULL PRIMARY KEY REFERENCES parents(id) \
                    NOT NULL PRIMARY KEY REFERENCES owners(id), \
                PRIMARY KEY (missing), \
                PRIMARY KEY (id)\
            )",
        );

        let CreateElement::Column(column) = &statement.elements[0] else {
            panic!("expected the first element to be a column");
        };
        assert_eq!(
            column.modifiers,
            vec![
                ColumnModifier::NotNull,
                ColumnModifier::PrimaryKey,
                ColumnModifier::References(ForeignKeyReference {
                    table: "parents".to_owned(),
                    column: "id".to_owned(),
                }),
                ColumnModifier::NotNull,
                ColumnModifier::PrimaryKey,
                ColumnModifier::References(ForeignKeyReference {
                    table: "owners".to_owned(),
                    column: "id".to_owned(),
                }),
            ]
        );
        assert_eq!(statement.elements.len(), 3);
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

    #[test]
    fn auto_increment_spellings_are_contextual_column_modifiers() {
        for modifier in ["AUTOINCREMENT", "AUTO_INCREMENT"] {
            let statement = create_table(&format!(
                "CREATE TABLE messages (id INTEGER PRIMARY KEY {modifier})"
            ));
            let CreateElement::Column(column) = &statement.elements[0] else {
                panic!("expected a column");
            };
            assert_eq!(
                column.modifiers,
                vec![ColumnModifier::PrimaryKey, ColumnModifier::AutoIncrement]
            );
        }

        let statement = create_table(
            "CREATE TABLE autoincrement (auto_increment INTEGER, value INTEGER PRIMARY KEY)",
        );
        assert_eq!(statement.table, "autoincrement");
        let CreateElement::Column(column) = &statement.elements[0] else {
            panic!("expected a column");
        };
        assert_eq!(column.name, "auto_increment");
        assert!(column.modifiers.is_empty());
    }

    #[test]
    fn preserves_duplicate_auto_increment_modifiers_for_resolution() {
        let statement = create_table(
            "CREATE TABLE ids (id INTEGER AUTOINCREMENT AUTO_INCREMENT AUTOINCREMENT)",
        );
        let CreateElement::Column(column) = &statement.elements[0] else {
            panic!("expected a column");
        };
        assert_eq!(
            column.modifiers,
            vec![
                ColumnModifier::AutoIncrement,
                ColumnModifier::AutoIncrement,
                ColumnModifier::AutoIncrement,
            ]
        );
    }
}
