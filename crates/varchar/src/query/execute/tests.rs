use super::{ByteBudget, map_regex_runtime, retained_row_charge};
use crate::storage::row_record;
use crate::value::Value;
use crate::{ErrorCode, Limits, Resource};
use fancy_regex::{Error as FancyError, RuntimeError};

#[test]
fn byte_budget_accepts_its_exact_bound_and_rejects_one_more_byte() {
    let mut budget = ByteBudget::new(10, Resource::QueryOutputBytes);

    budget.charge(4).expect("partial charge fits");
    budget.charge(6).expect("exact bound fits");
    let error = budget.charge(1).expect_err("one byte exceeds the budget");
    assert_eq!(error.resource(), Some(Resource::QueryOutputBytes));
    assert_eq!(error.limit(), Some(10));
    assert_eq!(budget.used, 10, "a rejected charge is not committed");
}

#[test]
fn transient_budget_checks_use_the_same_exact_boundary() {
    let mut budget = ByteBudget::new(10, Resource::QueryWorkingBytes);
    budget.charge(4).expect("retained working bytes fit");

    budget
        .check_transient(6)
        .expect("an exact transient peak fits");
    let error = budget
        .check_transient(7)
        .expect_err("the transient peak exceeds the budget");
    assert_eq!(error.resource(), Some(Resource::QueryWorkingBytes));
    assert_eq!(error.limit(), Some(10));
    assert_eq!(budget.used, 4, "transient checks do not retain a charge");
}

#[test]
fn retained_join_rows_charge_four_vector_descriptors() {
    let encoded = "~R|items|I1|Tok;";
    let row = row_record(encoded, 0).expect("fixture row is valid");
    let budget = ByteBudget::new(usize::MAX, Resource::QueryWorkingBytes);
    let expected =
        4 * std::mem::size_of::<Vec<Value>>() + 2 * std::mem::size_of::<Value>() + encoded.len();

    assert_eq!(
        retained_row_charge(&row, 2, &budget).expect("charge fits"),
        expected
    );
}

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
    assert_eq!(backtrack.code(), ErrorCode::ResourceLimit);
    assert_eq!(backtrack.resource(), Some(Resource::RegexBacktracking));
    assert_eq!(backtrack.limit(), Some(7));

    let stack = map_regex_runtime(
        FancyError::RuntimeError(RuntimeError::StackOverflow),
        &limits,
    );
    assert_eq!(stack.code(), ErrorCode::RegexRuntime);
    assert_eq!(stack.resource(), None);
    assert_eq!(stack.limit(), None);
}
