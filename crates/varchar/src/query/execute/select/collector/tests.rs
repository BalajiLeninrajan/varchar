use std::cmp::Ordering;

use super::{PendingRow, compare_pending, compare_values, ordered_row_charge};
use crate::query::execute::select::ByteBudget;
use crate::resolve::{ColumnLocation, ResolvedOrderTerm};
use crate::{Error, Resource, Value};

#[test]
fn ordered_row_charge_covers_descriptors_slots_ordinals_and_text() {
    let budget = ByteBudget::new(usize::MAX, Resource::QueryWorkingBytes);
    let projected_payload = 5;
    let key_payload = 7;
    let actual = ordered_row_charge(2, 3, projected_payload, key_payload, &budget)
        .expect("target-layout charge fits");
    let expected = 4 * std::mem::size_of::<PendingRow>()
        + 5 * std::mem::size_of::<Value>()
        + projected_payload
        + key_payload;

    assert_eq!(actual, expected);

    let mut exact = ByteBudget::new(actual + 11, Resource::QueryWorkingBytes);
    exact
        .charge_with_transient(actual, 11)
        .expect("descriptor and payload charge fits its exact peak");
    assert_eq!(exact.used, actual);

    let mut one_under = ByteBudget::new(actual + 10, Resource::QueryWorkingBytes);
    assert!(matches!(
        one_under.charge_with_transient(actual, 11),
        Err(Error::ResourceLimit {
            resource: Resource::QueryWorkingBytes,
            limit,
        }) if limit == actual + 10
    ));
    assert_eq!(one_under.used, 0);
}

#[test]
fn scalar_comparison_has_fixed_directional_null_placement() {
    assert_eq!(
        compare_values(&Value::Boolean(false), &Value::Boolean(true), false),
        Ordering::Less
    );
    assert_eq!(
        compare_values(&Value::Boolean(false), &Value::Boolean(true), true),
        Ordering::Greater
    );
    assert_eq!(
        compare_values(&Value::Null, &Value::Integer(1), false),
        Ordering::Greater
    );
    assert_eq!(
        compare_values(&Value::Null, &Value::Integer(1), true),
        Ordering::Less
    );
    assert_eq!(
        compare_values(
            &Value::Text("é".to_owned()),
            &Value::Text("💾".to_owned()),
            false,
        ),
        Ordering::Less
    );
}

#[test]
fn ordinal_is_the_final_comparison_key() {
    let terms = [ResolvedOrderTerm {
        column: ColumnLocation {
            source: 0,
            column: 0,
        },
        descending: true,
    }];
    let row = |ordinal| PendingRow {
        projected: vec![Value::Integer(ordinal as i64)],
        keys: vec![Value::Text("tie".to_owned())],
        ordinal,
    };

    assert_eq!(compare_pending(&row(1), &row(2), &terms), Ordering::Less);
}
