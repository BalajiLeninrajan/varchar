use super::{Error, ErrorCode, Span};
use crate::Resource;

#[test]
fn every_private_kind_maps_to_its_public_code() {
    let cases = [
        (
            Error::parse("message", Span::new(0, 1)),
            ErrorCode::SqlParse,
        ),
        (
            Error::unsupported("feature", Span::new(0, 1)),
            ErrorCode::UnsupportedSql,
        ),
        (Error::schema("message"), ErrorCode::Schema),
        (Error::type_error("message"), ErrorCode::Type),
        (Error::constraint("message"), ErrorCode::Constraint),
        (
            Error::corrupt_storage(0, "message"),
            ErrorCode::CorruptStorage,
        ),
        (Error::regex_compile("message"), ErrorCode::RegexCompile),
        (Error::regex_runtime("message"), ErrorCode::RegexRuntime),
        (Error::allocation("operation"), ErrorCode::Allocation),
        (Error::capacity("operation"), ErrorCode::Capacity),
        (
            Error::resource_limit(Resource::SqlBytes, 1),
            ErrorCode::ResourceLimit,
        ),
    ];

    for (error, code) in cases {
        assert_eq!(error.code(), code);
    }
}

#[test]
fn error_codes_expose_machine_names_and_human_displays() {
    let cases = [
        (ErrorCode::SqlParse, "sql_parse", "SQL parse error"),
        (
            ErrorCode::UnsupportedSql,
            "unsupported_sql",
            "unsupported SQL",
        ),
        (ErrorCode::Schema, "schema", "schema error"),
        (ErrorCode::Type, "type", "type error"),
        (ErrorCode::Constraint, "constraint", "constraint violation"),
        (
            ErrorCode::CorruptStorage,
            "corrupt_storage",
            "corrupt storage",
        ),
        (
            ErrorCode::RegexCompile,
            "regex_compile",
            "regex compilation error",
        ),
        (
            ErrorCode::RegexRuntime,
            "regex_runtime",
            "regex runtime error",
        ),
        (ErrorCode::ResourceLimit, "resource_limit", "resource limit"),
        (ErrorCode::Allocation, "allocation", "allocation failure"),
        (ErrorCode::Capacity, "capacity", "capacity exceeded"),
    ];

    for (code, machine_name, human_display) in cases {
        assert_eq!(code.as_str(), machine_name);
        assert_eq!(code.to_string(), human_display);
    }
}

#[test]
fn debug_output_uses_the_public_diagnostic_shape() {
    let debug = format!("{:?}", Error::schema("unknown table"));
    assert!(debug.contains("code: Schema"));
    assert!(debug.contains("detail: \"unknown table\""));
    assert!(!debug.contains("kind"));
}

#[test]
fn span_accessors_share_one_validated_representation() {
    let span = Span::new(2, 5);
    assert_eq!(span.start(), 2);
    assert_eq!(span.end(), 5);
    assert_eq!(span.len(), 3);
    assert!(!span.is_empty());
    assert_eq!(span.range(), 2..5);
    assert!(Span::new(4, 4).is_empty());
}

#[test]
#[should_panic(expected = "a SQL span must be ordered")]
fn span_construction_rejects_reversed_ranges() {
    let _ = Span::new(5, 4);
}
