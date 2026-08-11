//! `CREATE TABLE` schema, key, and reference grammar.

use super::{Parser, TokenKind};
use crate::sql::ast::{
    ColumnDef, ColumnModifier, CreateElement, CreateTable, ForeignKeyReference, TableConstraint,
};
use crate::{DataType, Error, Result};

impl Parser {
    pub(super) fn parse_create_table(&mut self) -> Result<CreateTable> {
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
            } else if self.current_word() == Some("UNIQUE") && self.peek_is(&TokenKind::LeftParen) {
                CreateElement::Constraint(self.parse_table_unique()?)
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
                Some("UNIQUE") => {
                    self.advance();
                    modifiers.push(ColumnModifier::Unique);
                }
                Some("REFERENCES") => {
                    modifiers.push(ColumnModifier::References(self.parse_reference()?));
                }
                Some("AUTOINCREMENT" | "AUTO_INCREMENT") => {
                    self.advance();
                    modifiers.push(ColumnModifier::AutoIncrement);
                }
                Some("DEFAULT") => {
                    self.advance();
                    modifiers.push(ColumnModifier::Default(self.parse_value()?));
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

    fn parse_table_unique(&mut self) -> Result<TableConstraint> {
        self.expect_keyword("UNIQUE")?;
        self.expect(TokenKind::LeftParen, "expected `(` after UNIQUE")?;
        let column = self.expect_identifier()?;
        self.reject_composite_constraint("UNIQUE")?;
        self.expect(TokenKind::RightParen, "expected `)` after UNIQUE column")?;
        Ok(TableConstraint::Unique(column))
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
}
