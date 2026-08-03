use super::{LexicalErrorKind, Token, TokenKind, lex_for_parser};
use crate::{Error, Span};

fn lexical_error(input: &str) -> (LexicalErrorKind, Span) {
    let tokens = lex_for_parser(input).expect("lexing defers contextual errors");
    let token = tokens
        .iter()
        .find(|token| matches!(token.kind, TokenKind::LexicalError(_)))
        .unwrap_or_else(|| panic!("expected a deferred lexical error for {input:?}"));
    let TokenKind::LexicalError(ref kind) = token.kind else {
        unreachable!("the located token is a lexical error");
    };
    (kind.clone(), token.span)
}

#[test]
fn tokens_retain_exact_utf8_byte_spans() {
    assert_eq!(
        lex_for_parser("select '\u{e9}''x', -7;").expect("SQL lexes"),
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
        lex_for_parser("users.id, posts.*").expect("qualified SQL lexes"),
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
fn deferred_lexical_errors_point_at_the_offending_bytes() {
    for (input, expected_kind, expected_span) in [
        (
            "\u{1f4a5}",
            LexicalErrorKind::UnexpectedCharacter('\u{1f4a5}'),
            Span::new(0, 4),
        ),
        (
            "'open",
            LexicalErrorKind::UnterminatedString,
            Span::new(0, 5),
        ),
        (
            "\"open",
            LexicalErrorKind::QuotedIdentifier,
            Span::new(0, 1),
        ),
        (
            "-- trailing",
            LexicalErrorKind::SqlComment,
            Span::new(0, 11),
        ),
        ("/* block", LexicalErrorKind::SqlComment, Span::new(0, 8)),
    ] {
        assert_eq!(
            lexical_error(input),
            (expected_kind, expected_span),
            "deferred lexical error for {input:?}"
        );
    }
}

#[test]
fn deferred_lexical_errors_regenerate_the_public_diagnostics() {
    assert!(matches!(
        LexicalErrorKind::UnexpectedCharacter('\u{1f4a5}').error(Span::new(0, 4)),
        Error::Parse {
            ref message,
            span_start: 0,
            span_end: 4,
        } if message == "unexpected character '\u{1f4a5}'"
    ));
    assert!(matches!(
        LexicalErrorKind::UnterminatedString.error(Span::new(0, 5)),
        Error::Parse {
            ref message,
            span_start: 0,
            span_end: 5,
        } if message == "unterminated string literal"
    ));
    assert!(matches!(
        LexicalErrorKind::QuotedIdentifier.error(Span::new(0, 1)),
        Error::Unsupported {
            ref feature,
            span_start: 0,
            span_end: 1,
        } if feature == "quoted identifiers"
    ));
    assert!(matches!(
        LexicalErrorKind::SqlComment.error(Span::new(0, 11)),
        Error::Unsupported {
            ref feature,
            span_start: 0,
            span_end: 11,
        } if feature == "SQL comments"
    ));
}

#[test]
fn a_lexical_error_stops_lexing_before_later_tokens() {
    assert_eq!(
        lex_for_parser("a @ b").expect("lexing defers contextual errors"),
        vec![
            Token {
                kind: TokenKind::Word(String::from("A")),
                span: Span::new(0, 1),
            },
            Token {
                kind: TokenKind::LexicalError(LexicalErrorKind::UnexpectedCharacter('@')),
                span: Span::new(2, 3),
            },
            Token {
                kind: TokenKind::End,
                span: Span::new(5, 5),
            },
        ]
    );
}

#[test]
fn bang_without_equals_is_still_an_eager_lexical_error() {
    assert!(matches!(
        lex_for_parser("!"),
        Err(Error::Parse {
            ref message,
            span_start: 0,
            span_end: 1,
        }) if message == "expected `=` after `!`"
    ));
}
