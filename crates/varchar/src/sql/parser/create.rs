//! `CREATE TABLE` schema, key, and reference grammar.

use super::{Parser, TokenKind};
use crate::sql::ast::{
    ColumnDef, ColumnModifier, CreateElement, CreateTable, ForeignKeyDeleteAction,
    ForeignKeyReference, ForeignKeyUpdateAction, TableConstraint,
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
            } else if self.current_word() == Some("CHECK") && self.peek_is(&TokenKind::LeftParen) {
                CreateElement::Constraint(TableConstraint::Check(self.parse_check_expression()?))
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
                Some("CHECK") if self.peek_is(&TokenKind::LeftParen) => {
                    modifiers.push(ColumnModifier::Check(self.parse_check_expression()?));
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

        let mut on_delete = None;
        let mut on_update = None;
        while self.current_word() == Some("ON")
            && matches!(self.peek_word(), Some("DELETE" | "UPDATE"))
        {
            let clause_span = self.current().span;
            self.advance();
            match self.current_word() {
                Some("DELETE") => {
                    self.advance();
                    if on_delete.is_some() {
                        return Err(Error::parse("duplicate ON DELETE clause", clause_span));
                    }
                    on_delete = Some(self.parse_on_delete_action()?);
                }
                Some("UPDATE") => {
                    self.advance();
                    if on_update.is_some() {
                        return Err(Error::parse("duplicate ON UPDATE clause", clause_span));
                    }
                    on_update = Some(self.parse_on_update_action()?);
                }
                _ => unreachable!("the loop guard recognizes the action clause"),
            }
        }

        Ok(ForeignKeyReference {
            table,
            column,
            on_delete: on_delete.unwrap_or_default(),
            on_update: on_update.unwrap_or_default(),
        })
    }

    fn parse_on_delete_action(&mut self) -> Result<ForeignKeyDeleteAction> {
        match self.current_word() {
            Some("RESTRICT") => {
                self.advance();
                Ok(ForeignKeyDeleteAction::Restrict)
            }
            Some("CASCADE") => {
                self.advance();
                Ok(ForeignKeyDeleteAction::Cascade)
            }
            Some("SET") => {
                self.advance();
                self.expect_keyword("NULL")?;
                Ok(ForeignKeyDeleteAction::SetNull)
            }
            _ => Err(Error::parse(
                "expected RESTRICT, CASCADE, or SET NULL after ON DELETE",
                self.current().span,
            )),
        }
    }

    fn parse_on_update_action(&mut self) -> Result<ForeignKeyUpdateAction> {
        match self.current_word() {
            Some("RESTRICT") => {
                self.advance();
                Ok(ForeignKeyUpdateAction::Restrict)
            }
            Some("CASCADE") => Err(Error::unsupported("ON UPDATE CASCADE", self.current().span)),
            _ => Err(Error::parse(
                "expected RESTRICT after ON UPDATE",
                self.current().span,
            )),
        }
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
