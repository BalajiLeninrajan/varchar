//! `INSERT`, `UPDATE`, and `DELETE` grammar.

use super::{Parser, TokenKind};
use crate::sql::ast::{Assignment, Delete, Insert, Update};
use crate::{Error, Result};

impl Parser {
    pub(super) fn parse_insert(&mut self) -> Result<Insert> {
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

    pub(super) fn parse_update(&mut self) -> Result<Update> {
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

    pub(super) fn parse_delete(&mut self) -> Result<Delete> {
        self.expect_keyword("DELETE")?;
        self.expect_keyword("FROM")?;
        let table = self.expect_identifier()?;
        let predicates = self.parse_optional_where()?;
        Ok(Delete { table, predicates })
    }
}
