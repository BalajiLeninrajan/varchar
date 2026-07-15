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
