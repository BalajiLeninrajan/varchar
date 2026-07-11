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
    Equal,
    NotEqual,
    Semicolon,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Token {
    pub(super) kind: TokenKind,
    pub(super) span: Span,
}

pub(super) fn lex(input: &str) -> Result<Vec<Token>> {
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
            '=' => {
                cursor += 1;
                TokenKind::Equal
            }
            ';' => {
                cursor += 1;
                TokenKind::Semicolon
            }
            '!' if bytes.get(cursor + 1) == Some(&b'=') => {
                cursor += 2;
                TokenKind::NotEqual
            }
            '!' => {
                return Err(Error::parse(
                    "expected `=` after `!`",
                    Span::new(start, start + 1),
                ));
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
                if !closed {
                    return Err(Error::parse(
                        "unterminated string literal",
                        Span::new(start, bytes.len()),
                    ));
                }
                TokenKind::String(value)
            }
            '"' => {
                return Err(Error::unsupported(
                    "quoted identifiers",
                    Span::new(start, start + 1),
                ));
            }
            '-' if bytes.get(cursor + 1) == Some(&b'-') => {
                return Err(Error::unsupported(
                    "SQL comments",
                    Span::new(start, bytes.len()),
                ));
            }
            '/' if bytes.get(cursor + 1) == Some(&b'*') => {
                return Err(Error::unsupported(
                    "SQL comments",
                    Span::new(start, bytes.len()),
                ));
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
            '<' | '>' => {
                return Err(Error::unsupported(
                    "ordered comparisons",
                    Span::new(start, start + width),
                ));
            }
            _ => {
                return Err(Error::parse(
                    format!("unexpected character {character:?}"),
                    Span::new(start, start + width),
                ));
            }
        };
        tokens.push(Token {
            kind,
            span: Span::new(start, cursor),
        });
    }

    tokens.push(Token {
        kind: TokenKind::End,
        span: Span::new(input.len(), input.len()),
    });
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::{Token, TokenKind, lex};
    use crate::{Error, Span};

    #[test]
    fn tokens_retain_exact_utf8_byte_spans() {
        assert_eq!(
            lex("select '\u{e9}''x', -7;").expect("SQL lexes"),
            vec![
                Token {
                    kind: TokenKind::Word(String::from("SELECT")),
                    span: Span::new(0, 6),
                },
                Token {
                    kind: TokenKind::String(String::from("\u{e9}'x")),
                    span: Span::new(7, 14),
                },
                Token {
                    kind: TokenKind::Comma,
                    span: Span::new(14, 15),
                },
                Token {
                    kind: TokenKind::Number(String::from("-7")),
                    span: Span::new(16, 18),
                },
                Token {
                    kind: TokenKind::Semicolon,
                    span: Span::new(18, 19),
                },
                Token {
                    kind: TokenKind::End,
                    span: Span::new(19, 19),
                },
            ]
        );
    }

    #[test]
    fn qualified_names_and_stars_retain_dot_spans() {
        assert_eq!(
            lex("users.id, posts.*").expect("qualified SQL lexes"),
            vec![
                Token {
                    kind: TokenKind::Word(String::from("USERS")),
                    span: Span::new(0, 5),
                },
                Token {
                    kind: TokenKind::Dot,
                    span: Span::new(5, 6),
                },
                Token {
                    kind: TokenKind::Word(String::from("ID")),
                    span: Span::new(6, 8),
                },
                Token {
                    kind: TokenKind::Comma,
                    span: Span::new(8, 9),
                },
                Token {
                    kind: TokenKind::Word(String::from("POSTS")),
                    span: Span::new(10, 15),
                },
                Token {
                    kind: TokenKind::Dot,
                    span: Span::new(15, 16),
                },
                Token {
                    kind: TokenKind::Star,
                    span: Span::new(16, 17),
                },
                Token {
                    kind: TokenKind::End,
                    span: Span::new(17, 17),
                },
            ]
        );
    }

    #[test]
    fn lexical_errors_point_at_the_offending_bytes() {
        assert!(matches!(
            lex("!"),
            Err(Error::Parse {
                ref message,
                span_start: 0,
                span_end: 1,
            }) if message == "expected `=` after `!`"
        ));
        assert!(matches!(
            lex("\u{1f4a5}"),
            Err(Error::Parse {
                ref message,
                span_start: 0,
                span_end: 4,
            }) if message == "unexpected character '\u{1f4a5}'"
        ));
        assert!(matches!(
            lex("'open"),
            Err(Error::Parse {
                ref message,
                span_start: 0,
                span_end: 5,
            }) if message == "unterminated string literal"
        ));
    }
}
