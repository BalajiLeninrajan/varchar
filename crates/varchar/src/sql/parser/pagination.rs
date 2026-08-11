//! `LIMIT` and `OFFSET` tail grammar with dedicated unsigned parsing.

use super::{Parser, TokenKind};
use crate::{Error, Result};

impl Parser {
    pub(super) fn parse_optional_pagination(&mut self) -> Result<(Option<u64>, Option<u64>)> {
        let limit = if self.consume_keyword("LIMIT") {
            Some(self.parse_pagination_integer("LIMIT")?)
        } else {
            None
        };
        let offset = if self.consume_keyword("OFFSET") {
            Some(self.parse_pagination_integer("OFFSET")?)
        } else {
            None
        };

        if limit.is_none() && offset.is_none() {
            return Ok((None, None));
        }
        if matches!(self.current().kind, TokenKind::End | TokenKind::Semicolon) {
            return Ok((limit, offset));
        }

        let message = match self.current_word() {
            Some("LIMIT") if offset.is_some() && limit.is_none() => "LIMIT must precede OFFSET",
            Some("LIMIT") => "duplicate LIMIT clause",
            Some("OFFSET") => "duplicate OFFSET clause",
            Some("ORDER") => "ORDER BY must precede LIMIT and OFFSET",
            _ if offset.is_some() => "no SQL syntax may follow OFFSET",
            _ => "expected OFFSET or the end of the SELECT statement",
        };
        Err(self.pagination_error(message))
    }

    fn parse_pagination_integer(&mut self, clause: &str) -> Result<u64> {
        let TokenKind::Number(value) = self.current().kind.clone() else {
            return Err(
                self.pagination_error(format!("expected an unsigned integer after {clause}"))
            );
        };
        if value.starts_with('-') {
            return Err(self.pagination_error(format!("{clause} requires an unsigned integer")));
        }
        let parsed = value
            .parse::<u64>()
            .map_err(|_| self.pagination_error(format!("{clause} is outside the u64 range")))?;
        self.advance();
        Ok(parsed)
    }

    fn pagination_error(&mut self, message: impl Into<String>) -> Error {
        if !matches!(
            self.current().kind,
            TokenKind::LexicalError(_)
                | TokenKind::ExpressionOperator(_)
                | TokenKind::Bang
                | TokenKind::LessThan
                | TokenKind::GreaterThan
        ) {
            self.claimed_pagination_error = Some(self.position);
        }
        Error::parse(message, self.current().span)
    }
}
