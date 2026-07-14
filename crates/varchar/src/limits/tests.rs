use super::{Limits, Resource, check_limit};
use crate::ErrorCode;

#[test]
fn defaults_cover_every_resource_bound() {
    assert_eq!(
        Limits::default(),
        Limits {
            max_database_bytes: 64 * 1024 * 1024,
            max_sql_bytes: 64 * 1024,
            max_predicates: 64,
            max_join_sources: 64,
            max_pattern_bytes: 8 * 1024 * 1024,
            max_query_working_bytes: 32 * 1024 * 1024,
            max_query_output_bytes: 32 * 1024 * 1024,
            max_join_steps: 1_000_000,
            regex_backtrack_limit: 1_000_000,
        }
    );
}

#[test]
fn check_limit_preserves_structured_resource_metadata() {
    assert!(check_limit(4, 4, Resource::JoinSteps).is_ok());

    let error = check_limit(5, 4, Resource::JoinSteps).expect_err("one over the limit fails");
    assert_eq!(error.code(), ErrorCode::ResourceLimit);
    assert_eq!(error.resource(), Some(Resource::JoinSteps));
    assert_eq!(error.limit(), Some(4));
}

#[test]
fn resources_expose_machine_names_and_human_displays() {
    let cases = [
        (Resource::DatabaseBytes, "database_bytes", "database bytes"),
        (Resource::SqlBytes, "sql_bytes", "SQL bytes"),
        (
            Resource::WherePredicates,
            "where_predicates",
            "WHERE predicates",
        ),
        (Resource::JoinSources, "join_sources", "JOIN sources"),
        (
            Resource::GeneratedRegexBytes,
            "generated_regex_bytes",
            "generated regex bytes",
        ),
        (
            Resource::QueryWorkingBytes,
            "query_working_bytes",
            "query working bytes",
        ),
        (
            Resource::QueryOutputBytes,
            "query_output_bytes",
            "query output bytes",
        ),
        (Resource::JoinSteps, "join_steps", "JOIN execution steps"),
        (
            Resource::RegexBacktracking,
            "regex_backtracking",
            "regex backtracking steps",
        ),
    ];

    for (resource, machine_name, human_display) in cases {
        assert_eq!(resource.as_str(), machine_name);
        assert_eq!(resource.to_string(), human_display);
    }
}
