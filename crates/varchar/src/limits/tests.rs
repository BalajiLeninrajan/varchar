use super::{Limits, Resource, check_limit};
use crate::Error;

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

    assert!(matches!(
        check_limit(5, 4, Resource::JoinSteps),
        Err(Error::ResourceLimit {
            resource: Resource::JoinSteps,
            limit: 4,
        })
    ));
}

#[test]
fn resources_have_human_readable_names() {
    let cases = [
        (Resource::DatabaseBytes, "database bytes"),
        (Resource::SqlBytes, "SQL bytes"),
        (Resource::WherePredicates, "WHERE predicates"),
        (Resource::CheckPredicates, "CHECK predicates"),
        (Resource::JoinSources, "JOIN sources"),
        (Resource::GeneratedRegexBytes, "generated regex bytes"),
        (Resource::QueryWorkingBytes, "query working bytes"),
        (Resource::QueryOutputBytes, "query output bytes"),
        (Resource::JoinSteps, "JOIN execution steps"),
        (Resource::RegexBacktracking, "regex backtracking steps"),
        (Resource::StorageWorkingBytes, "storage working bytes"),
    ];

    for (resource, human_display) in cases {
        assert_eq!(resource.to_string(), human_display);
    }
}
