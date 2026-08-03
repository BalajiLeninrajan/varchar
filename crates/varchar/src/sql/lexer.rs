//! Tokenization with byte spans and early rejection of unsupported syntax.

use crate::{Error, Result, Span};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TokenKind {
    Word(String),
    String(String),
    Number(String),
    LeftParen,
    RightParen,
    Comma,
    Dot,
    Star,
    Bang,
    Equal,
    NotEqual,
    LessThan,
    GreaterThan,
    LexicalError(LexicalErrorKind),
    Semicolon,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LexicalErrorKind {
    UnexpectedCharacter(char),
    UnterminatedString,
    QuotedIdentifier,
    SqlComment,
}

impl LexicalErrorKind {
    pub(super) fn error(&self, span: Span) -> Error {
        match self {
            Self::UnexpectedCharacter(character) => unexpected_character_error(*character, span),
            Self::UnterminatedString => Error::parse("unterminated string literal", span),
            Self::QuotedIdentifier => Error::unsupported("quoted identifiers", span),
            Self::SqlComment => Error::unsupported("SQL comments", span),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Token {
    pub(super) kind: TokenKind,
    pub(super) span: Span,
}

pub(super) fn lex_for_parser(input: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut cursor = 0;
    let bytes = input.as_bytes();

    while cursor < bytes.len() {
        let character = input[cursor..]
            .chars()
            .next()
            .expect("cursor is inside the input");
        let width = character.len_utf8();

        if character.is_whitespace() {
            cursor += width;
            continue;
        }

        let start = cursor;
        let kind = match character {
            '(' => {
                cursor += 1;
                TokenKind::LeftParen
            }
            ')' => {
                cursor += 1;
                TokenKind::RightParen
            }
            ',' => {
                cursor += 1;
                TokenKind::Comma
            }
            '.' => {
                cursor += 1;
                TokenKind::Dot
            }
            '*' => {
                cursor += 1;
                TokenKind::Star
            }
            '!' => {
                cursor += 1;
                if bytes.get(cursor) == Some(&b'=') {
                    cursor += 1;
                    TokenKind::NotEqual
                } else {
                    TokenKind::Bang
                }
            }
            '<' => {
                cursor += 1;
                TokenKind::LessThan
            }
            '=' => {
                cursor += 1;
                TokenKind::Equal
            }
            '>' => {
                cursor += 1;
                TokenKind::GreaterThan
            }
            ';' => {
                cursor += 1;
                TokenKind::Semicolon
            }
            '\'' => {
                cursor += 1;
                let mut value = String::new();
                let mut closed = false;
                while cursor < bytes.len() {
                    let next = input[cursor..]
                        .chars()
                        .next()
                        .expect("cursor is inside the input");
                    if next == '\'' {
                        if bytes.get(cursor + 1) == Some(&b'\'') {
                            value.push('\'');
                            cursor += 2;
                        } else {
                            cursor += 1;
                            closed = true;
                            break;
                        }
                    } else {
                        value.push(next);
                        cursor += next.len_utf8();
                    }
                }
                if closed {
                    TokenKind::String(value)
                } else {
                    TokenKind::LexicalError(LexicalErrorKind::UnterminatedString)
                }
            }
            '"' => {
                cursor += 1;
                TokenKind::LexicalError(LexicalErrorKind::QuotedIdentifier)
            }
            '-' if bytes.get(cursor + 1) == Some(&b'-') => {
                cursor = bytes.len();
                TokenKind::LexicalError(LexicalErrorKind::SqlComment)
            }
            '/' if bytes.get(cursor + 1) == Some(&b'*') => {
                cursor = bytes.len();
                TokenKind::LexicalError(LexicalErrorKind::SqlComment)
            }
            '-' if bytes.get(cursor + 1).is_some_and(u8::is_ascii_digit) => {
                cursor += 1;
                while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                    cursor += 1;
                }
                TokenKind::Number(input[start..cursor].to_owned())
            }
            value if value.is_ascii_digit() => {
                cursor += 1;
                while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
                    cursor += 1;
                }
                TokenKind::Number(input[start..cursor].to_owned())
            }
            value if value == '_' || value.is_ascii_alphabetic() => {
                cursor += 1;
                while bytes
                    .get(cursor)
                    .is_some_and(|byte| *byte == b'_' || byte.is_ascii_alphanumeric())
                {
                    cursor += 1;
                }
                TokenKind::Word(input[start..cursor].to_ascii_uppercase())
            }
            _ => {
                cursor += width;
                TokenKind::LexicalError(LexicalErrorKind::UnexpectedCharacter(character))
            }
        };
        let stops_lexing = matches!(&kind, TokenKind::LexicalError(_));
        tokens.push(Token {
            kind,
            span: Span::new(start, cursor),
        });
        if stops_lexing {
            break;
        }
    }

    tokens.push(Token {
        kind: TokenKind::End,
        span: Span::new(input.len(), input.len()),
    });
    Ok(tokens)
}

pub(super) fn unexpected_character_error(character: char, span: Span) -> Error {
    Error::parse(format!("unexpected character {character:?}"), span)
}

pub(super) fn comparison_error(operator: &str, span: Span) -> Error {
    match operator {
        "<>" => Error::unsupported("comparison operator `<>`", span),
        "!" => Error::parse("expected `=` after `!`", span),
        _ => Error::parse(format!("malformed comparison operator `{operator}`"), span),
    }
}

#[cfg(test)]
mod tests;
