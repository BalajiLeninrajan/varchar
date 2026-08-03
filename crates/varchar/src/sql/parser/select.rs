//! `SELECT`, projection, joins, and `EXPLAIN REGEX` grammar.

use super::{Parser, TokenKind};
use crate::sql::ast::{
    ColumnRef, Join, JoinCondition, OrderDirection, OrderTerm, Projection, ProjectionItem, Select,
};
use crate::{Error, Result};

impl Parser {
    pub(super) fn parse_select(&mut self) -> Result<Select> {
        self.expect_keyword("SELECT")?;
        let projection = self.parse_projection()?;
        self.expect_keyword("FROM")?;
        let table = self.expect_identifier()?;
        let joins = self.parse_joins()?;
        let where_clause = self.parse_optional_where()?;
        let order_by = self.parse_optional_order_by()?;
        Ok(Select {
            table,
            joins,
            projection,
            where_clause,
            order_by,
        })
    }

    fn parse_optional_order_by(&mut self) -> Result<Vec<OrderTerm>> {
        if !self.consume_keyword("ORDER") {
            return Ok(Vec::new());
        }
        self.expect_keyword("BY")?;

        let mut terms = Vec::new();
        loop {
            terms.try_reserve(1).map_err(|_| Error::Allocation {
                operation: "growing ORDER BY terms",
            })?;
            terms.push(self.parse_order_term()?);
            if self.consume(&TokenKind::Comma) {
                continue;
            }
            if self.order_by_is_terminated() {
                break;
            }

            let span = self.current().span;
            self.claimed_order_error = Some(self.position);
            return Err(Error::parse("expected `,` between ORDER BY terms", span));
        }
        Ok(terms)
    }

    fn parse_order_term(&mut self) -> Result<OrderTerm> {
        let column = self.parse_column_ref()?;
        let direction = if self.consume_keyword("ASC") {
            OrderDirection::Ascending
        } else if self.consume_keyword("DESC") {
            OrderDirection::Descending
        } else {
            OrderDirection::Ascending
        };

        Ok(OrderTerm { column, direction })
    }

    fn order_by_is_terminated(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::End | TokenKind::Semicolon | TokenKind::LexicalError(_)
        ) || self
            .current_word()
            .and_then(super::trailing_feature)
            .is_some()
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
