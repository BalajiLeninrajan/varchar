//! `SELECT`, projection, joins, and `EXPLAIN REGEX` grammar.

use super::{Parser, TokenKind};
use crate::sql::ast::{ColumnRef, Join, JoinCondition, Projection, ProjectionItem, Select};
use crate::{Error, Result};

impl Parser {
    pub(super) fn parse_select(&mut self) -> Result<Select> {
        self.expect_keyword("SELECT")?;
        let projection = self.parse_projection()?;
        self.expect_keyword("FROM")?;
        let table = self.expect_identifier()?;
        let joins = self.parse_joins()?;
        let where_clause = self.parse_optional_where()?;
        Ok(Select {
            table,
            joins,
            projection,
            where_clause,
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

    fn parse_joins(&mut self) -> Result<Vec<Join>> {
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

            let table = self.expect_identifier()?;
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

    pub(super) fn parse_explain(&mut self) -> Result<Select> {
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
}
