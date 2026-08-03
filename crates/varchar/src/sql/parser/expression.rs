//! Boolean-expression grammar and normalization.

use super::{Parser, TokenKind};
use crate::sql::ast::{Expression, ExpressionNode, Predicate, PredicateOperator};
use crate::{Error, Result};

impl Parser {
    pub(super) fn parse_optional_where(&mut self) -> Result<Option<Expression>> {
        if !self.consume_keyword("WHERE") {
            return Ok(None);
        }
        let mut predicates = vec![self.parse_predicate()?];
        while self.consume_keyword("AND") {
            predicates.push(self.parse_predicate()?);
        }
        if self.current_word() == Some("OR") {
            return Err(Error::unsupported("OR predicates", self.current().span));
        }
        normalize(predicates).map(Some)
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
}

/// Flatten a conjunction into one preorder program.
///
/// A lone predicate is its own root; two or more become an `And` root followed
/// by its leaves.
fn normalize(predicates: Vec<Predicate>) -> Result<Expression> {
    let children = predicates.len();
    let capacity = if children > 1 {
        children.checked_add(1).ok_or(Error::Capacity {
            operation: "counting WHERE predicates",
        })?
    } else {
        children
    };
    let mut nodes = Vec::new();
    nodes
        .try_reserve_exact(capacity)
        .map_err(|_| Error::Allocation {
            operation: "reserving the normalized expression program",
        })?;
    if children > 1 {
        nodes.push(ExpressionNode::And { children });
    }
    nodes.extend(predicates.into_iter().map(ExpressionNode::Predicate));
    Ok(Expression::new(nodes))
}
