use super::{ByteBudget, residual_evaluator, retained_row_charge};
use crate::expression::{Evaluator, Predicate, Program, ProgramNode};
use crate::resolve::ColumnLocation;
use crate::storage::row_record;
use crate::value::Value;
use crate::{Error, Resource};

#[test]
fn byte_budget_accepts_its_exact_bound_and_rejects_one_more_byte() {
    let mut budget = ByteBudget::new(10, Resource::QueryOutputBytes);

    budget.charge(4).expect("partial charge fits");
    budget.charge(6).expect("exact bound fits");
    assert!(matches!(
        budget.charge(1),
        Err(Error::ResourceLimit {
            resource: Resource::QueryOutputBytes,
            limit: 10,
        })
    ));
    assert_eq!(budget.used, 10, "a rejected charge is not committed");
}

#[test]
fn transient_budget_checks_use_the_same_exact_boundary() {
    let mut budget = ByteBudget::new(10, Resource::QueryWorkingBytes);
    budget.charge(4).expect("retained working bytes fit");

    budget
        .check_transient(6)
        .expect("an exact transient peak fits");
    assert!(matches!(
        budget.check_transient(7),
        Err(Error::ResourceLimit {
            resource: Resource::QueryWorkingBytes,
            limit: 10,
        })
    ));
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
fn one_evaluator_stack_is_charged_for_all_join_residuals() {
    let predicate = || {
        ProgramNode::Predicate(Predicate::IsNull {
            column: ColumnLocation {
                source: 0,
                column: 0,
            },
        })
    };
    let small = Program::new(vec![
        ProgramNode::Or { children: 2 },
        predicate(),
        predicate(),
    ]);
    let large = Program::new(vec![
        ProgramNode::Or { children: 2 },
        ProgramNode::And { children: 2 },
        predicate(),
        predicate(),
        predicate(),
    ]);
    let cross = Program::new(vec![
        ProgramNode::Or { children: 2 },
        predicate(),
        predicate(),
    ]);
    let expected = Evaluator::working_bytes(&large).expect("stack size fits");
    let local = [Some(small), Some(large)];

    let mut exact = ByteBudget::new(expected, Resource::QueryWorkingBytes);
    assert!(
        residual_evaluator(&local, Some(&cross), &mut exact, usize::MAX)
            .expect("largest reusable evaluator fits")
            .is_some()
    );
    assert_eq!(exact.used, expected);

    let mut one_under = ByteBudget::new(expected - 1, Resource::QueryWorkingBytes);
    assert!(matches!(
        residual_evaluator(&local, Some(&cross), &mut one_under, usize::MAX),
        Err(Error::ResourceLimit {
            resource: Resource::QueryWorkingBytes,
            limit,
        }) if limit == expected - 1
    ));
    assert_eq!(one_under.used, 0);
}
