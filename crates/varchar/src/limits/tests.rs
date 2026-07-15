use super::{Limits, check_limit};
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
fn check_limit_preserves_the_resource_label() {
    assert!(check_limit(4, 4, "rows").is_ok());
    assert!(matches!(
        check_limit(5, 4, "rows"),
        Err(Error::ResourceLimit {
            resource: "rows",
            limit: 4,
        })
    ));
}
