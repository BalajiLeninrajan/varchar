//! Stack-safe Boolean-expression grammar and normalization.

use super::{Parser, TokenKind};
use crate::sql::ast::{Expression, ExpressionNode, Predicate, PredicateOperator};
use crate::{Error, Result, Span, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogicalOperator {
    And,
    Or,
}

impl LogicalOperator {
    const fn precedence(self) -> u8 {
        match self {
            Self::Or => 1,
            Self::And => 2,
        }
    }
}

enum Operator {
    LeftParen,
    Logical(LogicalOperator),
}

enum TemporaryNode {
    Predicate(Predicate),
    Logical {
        operator: LogicalOperator,
        left: usize,
        right: usize,
    },
}

impl Parser {
    pub(super) fn parse_optional_where(&mut self) -> Result<Option<Expression>> {
        if !self.consume_keyword("WHERE") {
            return Ok(None);
        }

        let start = self.position;
        self.where_expression = Some(start..self.where_expression_end(start));
        self.parse_expression().map(Some)
    }

    fn where_expression_end(&self, start: usize) -> usize {
        let mut depth = 0_usize;
        let mut index = start;
        while let Some(token) = self.tokens.get(index) {
            if matches!(&token.kind, TokenKind::End | TokenKind::Semicolon)
                || (depth == 0 && self.starts_trailing_clause(index))
            {
                break;
            }
            match &token.kind {
                TokenKind::LeftParen => depth = depth.saturating_add(1),
                TokenKind::RightParen if depth > 0 => depth -= 1,
                _ => {}
            }
            index += 1;
        }
        index
    }

    fn starts_trailing_clause(&self, index: usize) -> bool {
        match self.word_at(index) {
            Some("JOIN" | "ORDER" | "GROUP" | "LIMIT" | "AS") => true,
            Some("LEFT" | "RIGHT" | "FULL") => {
                self.word_at(index + 1) == Some("JOIN")
                    || (self.word_at(index + 1) == Some("OUTER")
                        && self.word_at(index + 2) == Some("JOIN"))
            }
            Some("OUTER" | "CROSS" | "NATURAL") => self.word_at(index + 1) == Some("JOIN"),
            _ => false,
        }
    }

    fn parse_expression(&mut self) -> Result<Expression> {
        let mut operators = Vec::new();
        let mut values = Vec::new();
        let mut arena = Vec::new();
        let mut expecting_operand = true;

        loop {
            if expecting_operand {
                match &self.current().kind {
                    TokenKind::LeftParen => {
                        try_push(
                            &mut operators,
                            Operator::LeftParen,
                            "growing the expression operator stack",
                        )?;
                        self.advance();
                    }
                    TokenKind::RightParen => {
                        return Err(Error::parse(
                            "expected a predicate before `)`",
                            self.current().span,
                        ));
                    }
                    TokenKind::End | TokenKind::Semicolon => {
                        return Err(Error::parse(
                            "expected a predicate in WHERE",
                            self.current().span,
                        ));
                    }
                    TokenKind::Word(word) if word == "NOT" => {
                        return Err(Error::unsupported("unary NOT", self.current().span));
                    }
                    TokenKind::String(_) | TokenKind::Number(_) => {
                        return Err(Error::unsupported(
                            "literal-to-literal predicates",
                            self.current().span,
                        ));
                    }
                    TokenKind::Word(word) if matches!(word.as_str(), "TRUE" | "FALSE") => {
                        return Err(Error::unsupported(
                            "bare Boolean constants",
                            self.current().span,
                        ));
                    }
                    TokenKind::Word(word) if word == "NULL" => {
                        return Err(Error::unsupported(
                            "literal-to-literal predicates",
                            self.current().span,
                        ));
                    }
                    _ => {
                        let predicate = self.parse_predicate()?;
                        let index = arena.len();
                        try_push(
                            &mut arena,
                            Some(TemporaryNode::Predicate(predicate)),
                            "growing the parsed expression arena",
                        )?;
                        try_push(&mut values, index, "growing the expression value stack")?;
                        expecting_operand = false;
                    }
                }
                continue;
            }

            let incoming = match self.current_word() {
                Some("AND") => Some(LogicalOperator::And),
                Some("OR") => Some(LogicalOperator::Or),
                _ => None,
            };
            if let Some(incoming) = incoming {
                let span = self.current().span;
                self.advance();
                while matches!(
                    operators.last(),
                    Some(Operator::Logical(operator))
                        if operator.precedence() >= incoming.precedence()
                ) {
                    let Some(Operator::Logical(operator)) = operators.pop() else {
                        unreachable!("the matched operator is logical");
                    };
                    reduce(operator, &mut values, &mut arena, span)?;
                }
                try_push(
                    &mut operators,
                    Operator::Logical(incoming),
                    "growing the expression operator stack",
                )?;
                expecting_operand = true;
                continue;
            }

            if matches!(self.current().kind, TokenKind::RightParen) {
                let span = self.current().span;
                let mut found_left_paren = false;
                while let Some(operator) = operators.pop() {
                    match operator {
                        Operator::LeftParen => {
                            found_left_paren = true;
                            break;
                        }
                        Operator::Logical(operator) => {
                            reduce(operator, &mut values, &mut arena, span)?;
                        }
                    }
                }
                if !found_left_paren {
                    return Err(Error::parse("unmatched `)` in WHERE", span));
                }
                self.advance();
                continue;
            }

            if matches!(self.current().kind, TokenKind::End | TokenKind::Semicolon)
                || self
                    .current_word()
                    .is_some_and(|word| super::trailing_feature(word).is_some())
            {
                break;
            }

            return Err(Error::parse(
                "expected `AND`, `OR`, `)`, or the end of WHERE",
                self.current().span,
            ));
        }

        if expecting_operand {
            return Err(Error::parse(
                "expected a predicate after Boolean operator",
                self.current().span,
            ));
        }

        let span = self.current().span;
        while let Some(operator) = operators.pop() {
            match operator {
                Operator::LeftParen => {
                    return Err(Error::parse("expected `)` to close WHERE expression", span));
                }
                Operator::Logical(operator) => {
                    reduce(operator, &mut values, &mut arena, span)?;
                }
            }
        }

        let Some(root) = values.pop() else {
            return Err(Error::parse("expected a predicate in WHERE", span));
        };
        if !values.is_empty() {
            return Err(Error::parse(
                "expected a Boolean operator between predicates",
                span,
            ));
        }

        normalize(arena, root)
    }

    fn parse_predicate(&mut self) -> Result<Predicate> {
        let column = self.parse_column_ref()?;
        let operator = match self.current().kind.clone() {
            TokenKind::Equal => {
                self.advance();
                PredicateOperator::Equal(self.parse_predicate_value()?)
            }
            TokenKind::NotEqual => {
                self.advance();
                PredicateOperator::NotEqual(self.parse_predicate_value()?)
            }
            TokenKind::LessThan => {
                let inclusive = self.peek_is_adjacent(&TokenKind::Equal);
                self.advance();
                if inclusive {
                    self.advance();
                    PredicateOperator::LessThanOrEqual(self.parse_predicate_value()?)
                } else {
                    PredicateOperator::LessThan(self.parse_predicate_value()?)
                }
            }
            TokenKind::GreaterThan => {
                let inclusive = self.peek_is_adjacent(&TokenKind::Equal);
                self.advance();
                if inclusive {
                    self.advance();
                    PredicateOperator::GreaterThanOrEqual(self.parse_predicate_value()?)
                } else {
                    PredicateOperator::GreaterThan(self.parse_predicate_value()?)
                }
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
            TokenKind::Word(ref word) if word == "IN" && self.peek_is(&TokenKind::LeftParen) => {
                let keyword_span = self.current().span;
                self.advance();
                PredicateOperator::In(self.parse_in_values(keyword_span)?)
            }
            _ => {
                return Err(Error::parse(
                    "expected `=`, `!=`, `<`, `<=`, `>`, `>=`, `LIKE`, `IS [NOT] NULL`, or `IN (...)`",
                    self.current().span,
                ));
            }
        };
        Ok(Predicate { column, operator })
    }

    fn parse_in_values(&mut self, keyword_span: Span) -> Result<Vec<Value>> {
        self.expect(TokenKind::LeftParen, "expected `(` after IN")?;
        if matches!(self.current().kind, TokenKind::RightParen) {
            return Err(Error::unsupported("empty IN lists", keyword_span));
        }

        let mut values = Vec::new();
        loop {
            let value = self.parse_in_value()?;
            try_push(&mut values, value, "growing an IN literal list")?;
            if self.consume(&TokenKind::Comma) {
                continue;
            }
            if self.current_starts_in_list_expression() {
                return Err(self.unsupported_in_list_expression());
            }
            break;
        }
        self.expect(TokenKind::RightParen, "expected `)` after IN list")?;
        Ok(values)
    }

    fn parse_in_value(&mut self) -> Result<Value> {
        if self.current_word() == Some("SELECT") {
            self.claimed_in_expression = Some(self.position);
            return Err(Error::unsupported(
                "subqueries in IN lists",
                self.current().span,
            ));
        }
        if matches!(
            &self.current().kind,
            TokenKind::Word(word) if !matches!(word.as_str(), "TRUE" | "FALSE" | "NULL")
        ) || matches!(
            self.current().kind,
            TokenKind::LeftParen | TokenKind::ExpressionOperator(_)
        ) {
            return Err(self.unsupported_in_list_expression());
        }
        self.parse_value()
    }

    fn current_starts_in_list_expression(&self) -> bool {
        matches!(
            self.current().kind,
            TokenKind::Equal
                | TokenKind::NotEqual
                | TokenKind::LessThan
                | TokenKind::GreaterThan
                | TokenKind::Star
                | TokenKind::ExpressionOperator(_)
        ) || matches!(
            &self.current().kind,
            TokenKind::Number(value) if value.starts_with('-')
        ) || matches!(
            self.current_word(),
            Some("AND" | "OR" | "IS" | "LIKE" | "IN" | "BETWEEN" | "NOT" | "COLLATE")
        )
    }

    fn unsupported_in_list_expression(&mut self) -> Error {
        let token = self.current();
        let span = match &token.kind {
            TokenKind::Number(value) if value.starts_with('-') => {
                Span::new(token.span.start, token.span.start + 1)
            }
            _ => token.span,
        };
        self.claimed_in_expression = Some(self.position);
        Error::unsupported("expressions in IN lists", span)
    }

    fn parse_predicate_value(&mut self) -> Result<Value> {
        if matches!(self.current().kind, TokenKind::Word(ref word) if !super::is_reserved(word)) {
            return Err(Error::unsupported(
                "column-to-column WHERE predicates",
                self.current().span,
            ));
        }
        self.parse_value()
    }
}

fn reduce(
    operator: LogicalOperator,
    values: &mut Vec<usize>,
    arena: &mut Vec<Option<TemporaryNode>>,
    span: Span,
) -> Result<()> {
    let Some(right) = values.pop() else {
        return Err(Error::parse(
            "Boolean operator is missing a right operand",
            span,
        ));
    };
    let Some(left) = values.pop() else {
        return Err(Error::parse(
            "Boolean operator is missing a left operand",
            span,
        ));
    };
    let index = arena.len();
    try_push(
        arena,
        Some(TemporaryNode::Logical {
            operator,
            left,
            right,
        }),
        "growing the parsed expression arena",
    )?;
    try_push(values, index, "growing the expression value stack")
}

fn normalize(mut arena: Vec<Option<TemporaryNode>>, root: usize) -> Result<Expression> {
    let capacity = arena.len();
    let mut nodes = Vec::new();
    let mut pending = Vec::new();
    let mut flatten = Vec::new();
    let mut children = Vec::new();
    reserve_exact(
        &mut nodes,
        capacity,
        "reserving the normalized expression program",
    )?;
    reserve_exact(
        &mut pending,
        capacity,
        "reserving the expression traversal stack",
    )?;
    reserve_exact(
        &mut flatten,
        capacity,
        "reserving the associative-flattening stack",
    )?;
    reserve_exact(
        &mut children,
        capacity,
        "reserving normalized expression children",
    )?;
    pending.push(root);

    while let Some(index) = pending.pop() {
        let operator = match arena[index]
            .as_ref()
            .expect("every pending expression node is present")
        {
            TemporaryNode::Predicate(_) => None,
            TemporaryNode::Logical { operator, .. } => Some(*operator),
        };

        let Some(operator) = operator else {
            let Some(TemporaryNode::Predicate(predicate)) = arena[index].take() else {
                unreachable!("the inspected node is a predicate");
            };
            nodes.push(ExpressionNode::Predicate(predicate));
            continue;
        };

        flatten.clear();
        children.clear();
        flatten.push(index);
        while let Some(candidate) = flatten.pop() {
            match arena[candidate]
                .as_ref()
                .expect("reachable expression nodes remain present")
            {
                TemporaryNode::Logical {
                    operator: candidate_operator,
                    left,
                    right,
                } if *candidate_operator == operator => {
                    flatten.push(*right);
                    flatten.push(*left);
                }
                TemporaryNode::Predicate(_) | TemporaryNode::Logical { .. } => {
                    children.push(candidate);
                }
            }
        }

        debug_assert!(children.len() >= 2);
        nodes.push(match operator {
            LogicalOperator::And => ExpressionNode::And {
                children: children.len(),
            },
            LogicalOperator::Or => ExpressionNode::Or {
                children: children.len(),
            },
        });
        pending.extend(children.iter().rev().copied());
    }

    Ok(Expression::new(nodes))
}

fn try_push<T>(values: &mut Vec<T>, value: T, operation: &'static str) -> Result<()> {
    values
        .try_reserve(1)
        .map_err(|_| Error::Allocation { operation })?;
    values.push(value);
    Ok(())
}

fn reserve_exact<T>(values: &mut Vec<T>, additional: usize, operation: &'static str) -> Result<()> {
    values
        .try_reserve_exact(additional)
        .map_err(|_| Error::Allocation { operation })
}
