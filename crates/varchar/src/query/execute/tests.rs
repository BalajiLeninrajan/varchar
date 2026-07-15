use super::map_regex_runtime;
use crate::{Error, Limits, Resource};
use fancy_regex::{Error as FancyError, RuntimeError};

#[test]
fn only_backtrack_exhaustion_is_a_configured_resource_limit() {
    let limits = Limits {
        regex_backtrack_limit: 7,
        ..Limits::default()
    };
    let backtrack = map_regex_runtime(
        FancyError::RuntimeError(RuntimeError::BacktrackLimitExceeded),
        &limits,
    );
    assert!(matches!(
        backtrack,
        Error::ResourceLimit {
            resource: Resource::RegexBacktracking,
            limit: 7,
        }
    ));

    let stack = map_regex_runtime(
        FancyError::RuntimeError(RuntimeError::StackOverflow),
        &limits,
    );
    assert!(matches!(stack, Error::RegexRuntime(_)));
}
