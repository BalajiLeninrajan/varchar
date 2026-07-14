use super::{Token, TokenKind, lex};
use crate::{ErrorCode, Span};

fn assert_lex_error(sql: &str, detail: &str, span: Span) {
    let error = lex(sql).expect_err("SQL should fail lexing");
    assert_eq!(error.code(), ErrorCode::SqlParse);
    assert_eq!(error.span(), Some(span));
    assert_eq!(
        error.to_string(),
        format!(
            "SQL parse error at bytes {}..{}: {detail}",
            span.start(),
            span.end()
        )
    );
}

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
    assert_lex_error("!", "expected `=` after `!`", Span::new(0, 1));
    assert_lex_error(
        "\u{1f4a5}",
        "unexpected character '\u{1f4a5}'",
        Span::new(0, 4),
    );
    assert_lex_error("'open", "unterminated string literal", Span::new(0, 5));
}
