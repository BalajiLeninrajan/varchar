use std::cmp::Ordering;

use super::{
    CollectionStatus, PendingRow, collect_ordered, collect_streaming, compare_pending,
    compare_values, ordered_retention, ordered_row_charge, ordered_window, row_structure_charge,
};
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

#[test]
fn streaming_pagination_skips_before_output_charge_and_stops_at_limit() {
    let source = vec![Value::Text("payload".to_owned())];
    let sources = [source.as_slice()];
    let projection = [ColumnLocation {
        source: 0,
        column: 0,
    }];
    let mut rows = Vec::new();
    let mut skipped = 0;
    let mut emitted = 0;
    let mut zero_budget = ByteBudget::new(0, Resource::QueryOutputBytes);

    assert_eq!(
        collect_streaming(
            &mut rows,
            &mut skipped,
            &mut emitted,
            Some(1),
            1,
            &projection,
            &sources,
            0,
            &mut zero_budget,
        )
        .expect("OFFSET skips without materializing"),
        CollectionStatus::Continue
    );
    assert_eq!(zero_budget.used, 0);
    assert!(rows.is_empty());

    let mut output_budget = ByteBudget::new(usize::MAX, Resource::QueryOutputBytes);
    let row_structure =
        row_structure_charge(projection.len(), &output_budget).expect("row charge fits");
    assert_eq!(
        collect_streaming(
            &mut rows,
            &mut skipped,
            &mut emitted,
            Some(1),
            1,
            &projection,
            &sources,
            row_structure,
            &mut output_budget,
        )
        .expect("first emitted row fits"),
        CollectionStatus::Complete
    );
    assert_eq!(rows, vec![vec![Value::Text("payload".to_owned())]]);

    let charged = output_budget.used;
    assert_eq!(
        collect_streaming(
            &mut rows,
            &mut skipped,
            &mut emitted,
            Some(1),
            1,
            &projection,
            &sources,
            row_structure,
            &mut output_budget,
        )
        .expect("completed LIMIT remains complete"),
        CollectionStatus::Complete
    );
    assert_eq!(output_budget.used, charged);
    assert_eq!(rows.len(), 1);
}

#[test]
fn retention_bounds_the_window_and_falls_back_to_unbounded_collection() {
    assert_eq!(ordered_retention(0, None), None);
    assert_eq!(ordered_retention(u64::MAX, None), None);
    assert_eq!(ordered_retention(0, Some(10)), Some(10));
    assert_eq!(ordered_retention(3, Some(1)), Some(4));
    // An empty window is empty at every offset.
    assert_eq!(ordered_retention(0, Some(0)), Some(0));
    assert_eq!(ordered_retention(u64::MAX, Some(0)), Some(0));
    // A bound no `Vec` could ever reach is treated as unbounded rather than
    // silently truncating the window.
    assert_eq!(ordered_retention(u64::MAX, Some(1)), None);
}

#[test]
fn bounded_ordered_collection_keeps_its_window_and_refunds_evictions() {
    let key = ColumnLocation {
        source: 0,
        column: 0,
    };
    let terms = [ResolvedOrderTerm {
        column: key,
        descending: false,
    }];
    let projection = [key];
    let mut rows = Vec::new();
    let mut next_ordinal = 0;
    let mut budget = ByteBudget::new(usize::MAX, Resource::QueryWorkingBytes);
    let mut peak = 0;

    // Ties on the sort key must keep the earlier ordinal, exactly as the final
    // stable sort would, so the two `1`s here are not interchangeable.
    for scanned in [5_i64, 1, 4, 1, 3, 2] {
        let source = [Value::Integer(scanned)];
        let sources = [source.as_slice()];
        assert_eq!(
            collect_ordered(
                &mut rows,
                &mut next_ordinal,
                &projection,
                &terms,
                Some(2),
                &sources,
                &mut budget,
                0,
            )
            .expect("a bounded window always has room"),
            CollectionStatus::Continue
        );
        assert!(rows.len() <= 2, "retention never exceeds OFFSET + LIMIT");
        peak = peak.max(budget.used);
    }
    assert_eq!(next_ordinal, 6, "every scanned row consumes an ordinal");

    rows.sort_unstable_by(|left, right| compare_pending(left, right, &terms));
    assert_eq!(
        rows.iter()
            .map(|row| (row.projected.clone(), row.ordinal))
            .collect::<Vec<_>>(),
        vec![(vec![Value::Integer(1)], 1), (vec![Value::Integer(1)], 3)]
    );

    let live = ordered_row_charge(1, 1, 0, 0, &budget).expect("target-layout charge fits");
    assert_eq!(
        (budget.used, peak),
        (2 * live, 2 * live),
        "only the retained window is ever charged"
    );
}

#[test]
fn an_empty_ordered_window_completes_without_retaining_anything() {
    let key = ColumnLocation {
        source: 0,
        column: 0,
    };
    let terms = [ResolvedOrderTerm {
        column: key,
        descending: false,
    }];
    let source = [Value::Integer(1)];
    let mut rows = Vec::new();
    let mut next_ordinal = 0;
    let mut zero_budget = ByteBudget::new(0, Resource::QueryWorkingBytes);

    assert_eq!(
        collect_ordered(
            &mut rows,
            &mut next_ordinal,
            &[key],
            &terms,
            Some(0),
            &[source.as_slice()],
            &mut zero_budget,
            0,
        )
        .expect("an empty window charges nothing"),
        CollectionStatus::Complete
    );
    assert!(rows.is_empty());
    assert_eq!((next_ordinal, zero_budget.used), (0, 0));
}

#[test]
fn ordered_windows_clamp_u64_bounds_before_usize_conversion() {
    assert_eq!(ordered_window(3, u64::MAX, None).unwrap(), (3, 0));
    assert_eq!(ordered_window(3, 1, Some(u64::MAX)).unwrap(), (1, 2));
    assert_eq!(ordered_window(3, 1, Some(0)).unwrap(), (1, 0));
}
