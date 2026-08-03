use std::cell::Cell;

use super::model::{
    FrozenRow, PreparedDirectUpdate, RowIdentity, decoded_values_bytes,
    set_value_clone_failure_after,
};
use super::referential::{ReferentialAction, ReferentialIndex};
use super::{
    defer_auto_increment, freeze_rows, measure_and_check_update_database_size, push_update_queue,
    sequence_edit_lengths_for_targets, sort_and_validate_ranges,
};
use crate::limits::ByteBudget;
use crate::storage::{RowLayout, StorageState, validate_row_layout, with_validated_row_encoder};
use crate::{DataType, Error, Resource, SchemaColumn, Value};

fn integer_columns() -> Vec<SchemaColumn> {
    vec![SchemaColumn {
        name: String::from("id"),
        data_type: DataType::Integer,
        nullable: false,
        default: None,
    }]
}

#[test]
fn original_ranges_are_decoded_and_evaluated_once() {
    let first = "~R|t|I1;";
    let second = "~R|t|I2;";
    let blob = format!("{first}{second}");
    let columns = integer_columns();
    let layout = RowLayout {
        table: "t",
        columns: &columns,
    };
    let ranges = [Ok(0..first.len()), Ok(first.len()..blob.len())];
    let mut budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);
    let mut evaluations = 0;

    let (rows, direct_affected) = freeze_rows(&blob, ranges, layout, &mut budget, |values| {
        evaluations += 1;
        Ok(values[0] == Value::Integer(1))
    })
    .expect("the validated records freeze");

    assert_eq!(evaluations, 2);
    assert_eq!(direct_affected, 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].identity().range(), 0..first.len());
    assert_eq!(rows[0].original_values(), &[Value::Integer(1)]);
}

#[test]
fn sequence_lengths_are_requested_once_only_for_nonempty_targets() {
    let calls = Cell::new(0);
    assert_eq!(
        sequence_edit_lengths_for_targets(0, || {
            calls.set(calls.get() + 1);
            Ok(Some((1, 2)))
        })
        .expect("zero targets need no sequence lengths"),
        None
    );
    assert_eq!(calls.get(), 0);

    assert_eq!(
        sequence_edit_lengths_for_targets(1, || {
            calls.set(calls.get() + 1);
            Ok(Some((1, 2)))
        })
        .expect("matching targets request sequence lengths"),
        Some((1, 2))
    );
    assert_eq!(calls.get(), 1);
}

#[test]
fn physical_ranges_are_sorted_and_overlaps_are_rejected() {
    let mut rows = vec![
        FrozenRow::new(
            RowIdentity::new(30..40).expect("valid identity"),
            Vec::new(),
        ),
        FrozenRow::new(
            RowIdentity::new(10..20).expect("valid identity"),
            Vec::new(),
        ),
        FrozenRow::new(
            RowIdentity::new(20..30).expect("valid identity"),
            Vec::new(),
        ),
    ];
    sort_and_validate_ranges(&mut rows).expect("adjacent edits do not overlap");
    assert_eq!(
        rows.iter()
            .map(|row| (row.identity().start(), row.identity().end()))
            .collect::<Vec<_>>(),
        vec![(10, 20), (20, 30), (30, 40)]
    );

    rows.push(FrozenRow::new(
        RowIdentity::new(19..21).expect("valid identity"),
        Vec::new(),
    ));
    assert!(matches!(
        sort_and_validate_ranges(&mut rows),
        Err(Error::CorruptStorage { offset: 19, .. })
    ));
}

#[test]
fn direct_overlays_preserve_original_values_and_detect_conflicts() {
    let columns = vec![
        SchemaColumn {
            name: String::from("id"),
            data_type: DataType::Integer,
            nullable: false,
            default: None,
        },
        SchemaColumn {
            name: String::from("body"),
            data_type: DataType::Text,
            nullable: false,
            default: None,
        },
    ];
    let layout = RowLayout {
        table: "t",
        columns: &columns,
    };
    let identity = RowIdentity::new(11..22).expect("valid identity");
    let mut row = FrozenRow::new(
        identity,
        vec![Value::Integer(1), Value::Text(String::from("old"))],
    );
    let mut assignments = vec![(1, Value::Text(String::from("a longer value")))];
    let validated_layout = validate_row_layout(layout).expect("valid layout");
    let update =
        PreparedDirectUpdate::new(&mut assignments, validated_layout.column_count(), identity)
            .expect("valid direct update");
    let mut budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);

    with_validated_row_encoder(validated_layout, |encoder| {
        let measured = row
            .measure_direct_update(&update, &encoder)
            .expect("the direct update measures");
        row.install_direct_update(&update, &mut budget)
            .expect("the first direct overlay installs");
        assert_eq!(
            row.original_values(),
            &[Value::Integer(1), Value::Text(String::from("old"))]
        );
        assert_eq!(row.identity(), identity);
        row.encode_effective_update(&encoder, measured, &mut budget)
            .expect("the effective row encodes");
    });
    assert_eq!(
        row.replacement().expect("replacement is planned"),
        Some("~R|t|I1|Ta longer value;")
    );

    assert!(matches!(
        row.install_direct_update(&update, &mut budget),
        Err(Error::Constraint(_))
    ));

    let mut updated = FrozenRow::new(identity, vec![Value::Integer(1), Value::Null]);
    with_validated_row_encoder(validated_layout, |encoder| {
        updated
            .measure_direct_update(&update, &encoder)
            .expect("the direct update measures");
    });
    updated
        .install_direct_update(&update, &mut budget)
        .expect("the direct update installs");
    assert!(matches!(
        updated.request_delete(&mut budget),
        Err(Error::Constraint(_))
    ));

    let mut deleted = FrozenRow::new(identity, vec![Value::Integer(1), Value::Null]);
    assert!(
        deleted
            .request_delete(&mut budget)
            .expect("the delete installs")
    );
    assert!(matches!(
        deleted.install_direct_update(&update, &mut budget),
        Err(Error::Constraint(_))
    ));
}

#[test]
fn prepared_updates_sort_once_and_merge_sparse_assignments_canonically() {
    let columns = vec![
        SchemaColumn {
            name: String::from("id"),
            data_type: DataType::Integer,
            nullable: false,
            default: None,
        },
        SchemaColumn {
            name: String::from("body"),
            data_type: DataType::Text,
            nullable: false,
            default: None,
        },
        SchemaColumn {
            name: String::from("active"),
            data_type: DataType::Boolean,
            nullable: false,
            default: None,
        },
    ];
    let layout = validate_row_layout(RowLayout {
        table: "items",
        columns: &columns,
    })
    .expect("valid layout");
    let identity = RowIdentity::new(5..25).expect("valid identity");
    let mut assignments = vec![(2, Value::Boolean(true)), (0, Value::Integer(9))];
    let update = PreparedDirectUpdate::new(&mut assignments, layout.column_count(), identity)
        .expect("reverse-ordered assignments prepare");
    let mut row = FrozenRow::new(
        identity,
        vec![
            Value::Integer(1),
            Value::Text(String::from("kept")),
            Value::Boolean(false),
        ],
    );
    let mut budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);

    with_validated_row_encoder(layout, |encoder| {
        let measured = row
            .measure_direct_update(&update, &encoder)
            .expect("sparse update measures");
        row.install_direct_update(&update, &mut budget)
            .expect("sorted overlays install");
        row.encode_effective_update(&encoder, measured, &mut budget)
            .expect("sparse update encodes");
    });
    assert_eq!(
        row.replacement().expect("replacement is planned"),
        Some("~R|items|I9|Tkept|B1;")
    );
}

#[test]
fn prepared_updates_reject_duplicate_and_out_of_range_columns() {
    let identity = RowIdentity::new(1..2).expect("valid identity");
    let mut duplicate = vec![(0, Value::Integer(1)), (0, Value::Integer(2))];
    assert!(matches!(
        PreparedDirectUpdate::new(&mut duplicate, 1, identity),
        Err(Error::Constraint(_))
    ));

    let mut out_of_range = vec![(1, Value::Integer(1))];
    assert!(matches!(
        PreparedDirectUpdate::new(&mut out_of_range, 1, identity),
        Err(Error::Schema(message)) if message == "UPDATE assignment column 1 is outside a frozen row"
    ));
}

#[test]
fn row_mutation_states_reject_incomplete_or_conflicting_transitions() {
    let columns = vec![SchemaColumn {
        name: String::from("id"),
        data_type: DataType::Integer,
        nullable: false,
        default: None,
    }];
    let layout = validate_row_layout(RowLayout {
        table: "items",
        columns: &columns,
    })
    .expect("valid layout");
    let identity = RowIdentity::new(2..10).expect("valid identity");
    let mut assignments = vec![(0, Value::Integer(2))];
    let update = PreparedDirectUpdate::new(&mut assignments, layout.column_count(), identity)
        .expect("valid update");
    let mut budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);
    let mut row = FrozenRow::new(identity, vec![Value::Integer(1)]);

    assert!(matches!(row.replacement(), Err(Error::Capacity { .. })));
    with_validated_row_encoder(layout, |encoder| {
        let measured = row
            .measure_direct_update(&update, &encoder)
            .expect("update measures");
        assert!(matches!(row.replacement(), Err(Error::Capacity { .. })));
        assert!(matches!(
            row.encode_effective_update(&encoder, measured, &mut budget),
            Err(Error::Constraint(_))
        ));
    });
    row.install_direct_update(&update, &mut budget)
        .expect("update installs");
    assert!(matches!(row.replacement(), Err(Error::Capacity { .. })));

    let mut deleted = FrozenRow::new(identity, vec![Value::Integer(1)]);
    assert!(
        deleted
            .request_delete(&mut budget)
            .expect("delete installs")
    );
    with_validated_row_encoder(layout, |encoder| {
        assert!(matches!(
            deleted.measure_direct_update(&update, &encoder),
            Err(Error::Constraint(_))
        ));
    });
}

#[test]
fn set_null_columns_are_sorted_once_before_encoding() {
    let columns = (0..4)
        .map(|index| SchemaColumn {
            name: format!("c{index}"),
            data_type: DataType::Integer,
            nullable: true,
            default: None,
        })
        .collect::<Vec<_>>();
    let layout = validate_row_layout(RowLayout {
        table: "items",
        columns: &columns,
    })
    .expect("valid layout");
    let identity = RowIdentity::new(0..20).expect("valid identity");
    let mut row = FrozenRow::new(
        identity,
        vec![
            Value::Integer(0),
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
        ],
    );
    let mut budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);
    for column in [3, 2, 1, 0, 2] {
        row.request_set_null(column, &mut budget)
            .expect("SET NULL request succeeds");
    }

    with_validated_row_encoder(layout, |encoder| {
        row.encode_set_null(&encoder, &mut budget)
            .expect("SET NULL row encodes");
    });
    assert_eq!(
        row.replacement().expect("replacement is planned"),
        Some("~R|items|N|N|N|N;")
    );
}

#[test]
fn storage_working_budget_releases_overlays_at_exact_multi_row_boundaries() {
    fn plan_two_updates(limit: usize) -> crate::Result<(usize, usize)> {
        let columns = vec![
            SchemaColumn {
                name: String::from("id"),
                data_type: DataType::Integer,
                nullable: false,
                default: None,
            },
            SchemaColumn {
                name: String::from("body"),
                data_type: DataType::Text,
                nullable: false,
                default: None,
            },
        ];
        let layout = validate_row_layout(RowLayout {
            table: "t",
            columns: &columns,
        })?;
        let original_record = "~R|t|I1|Told;";
        let mut assignments = vec![(1, Value::Text(String::from("longer")))];
        let first_identity = RowIdentity::new(0..original_record.len())?;
        let update =
            PreparedDirectUpdate::new(&mut assignments, layout.column_count(), first_identity)?;
        let mut budget = ByteBudget::new(limit, Resource::StorageWorkingBytes);
        let mut rows = Vec::new();
        let _ = budget.reserve_exact(&mut rows, 2, "reserving test targets")?;
        for index in 0..2 {
            let start = index * original_record.len();
            let decoded = decoded_values_bytes(columns.len(), original_record.len(), &budget)?;
            budget.charge(decoded)?;
            rows.push(FrozenRow::new(
                RowIdentity::new(start..start + original_record.len())?,
                vec![Value::Integer(1), Value::Text(String::from("old"))],
            ));
        }

        with_validated_row_encoder(layout, |encoder| {
            let (measurements, measurement_working_bytes) = measure_and_check_update_database_size(
                original_record.len() * 2,
                usize::MAX,
                &mut rows,
                &encoder,
                &update,
                None,
                &mut budget,
            )?;
            let encoded_len = measurements[0].encoded_len();
            assert_eq!(measurements[1].encoded_len(), encoded_len);
            let after_measurement = budget.used;

            rows[0].install_direct_update(&update, &mut budget)?;
            let one_overlay = budget.used - after_measurement;
            rows[1].install_direct_update(&update, &mut budget)?;
            assert_eq!(budget.used - after_measurement, one_overlay * 2);
            let all_overlays = budget.used;
            let exact_peak =
                (all_overlays + encoded_len).max(all_overlays - one_overlay + encoded_len * 2);

            for (row, measured) in rows.iter_mut().zip(measurements) {
                row.encode_effective_update(&encoder, measured, &mut budget)?;
            }
            budget.release(measurement_working_bytes);
            Ok((budget.used, exact_peak))
        })
    }

    let (final_used, exact) =
        plan_two_updates(usize::MAX).expect("the exact multi-row peak is measurable");
    assert!(
        final_used < exact,
        "released overlays must not remain charged"
    );
    assert_eq!(
        plan_two_updates(exact)
            .expect("the exact storage-working bound succeeds")
            .0,
        final_used
    );
    assert!(matches!(
        plan_two_updates(exact - 1),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit,
        }) if limit == exact - 1
    ));
}

#[test]
fn overlay_installation_refunds_failed_payload_and_clone_charges() {
    let columns = vec![
        SchemaColumn {
            name: String::from("id"),
            data_type: DataType::Integer,
            nullable: false,
            default: None,
        },
        SchemaColumn {
            name: String::from("body"),
            data_type: DataType::Text,
            nullable: false,
            default: None,
        },
    ];
    let layout = validate_row_layout(RowLayout {
        table: "items",
        columns: &columns,
    })
    .expect("valid layout");
    let identity = RowIdentity::new(0..20).expect("valid identity");
    let long_text = "x".repeat(1_024);

    let mut payload_assignments = vec![(1, Value::Text(long_text.clone()))];
    let payload_update =
        PreparedDirectUpdate::new(&mut payload_assignments, layout.column_count(), identity)
            .expect("payload update prepares");
    let mut probe_row = FrozenRow::new(
        identity,
        vec![Value::Integer(1), Value::Text(String::from("old"))],
    );
    with_validated_row_encoder(layout, |encoder| {
        probe_row
            .measure_direct_update(&payload_update, &encoder)
            .expect("probe update measures");
    });
    let mut probe_budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);
    probe_row
        .install_direct_update(&payload_update, &mut probe_budget)
        .expect("probe overlay installs");
    let installed_bytes = probe_budget.used;

    let mut payload_row = FrozenRow::new(
        identity,
        vec![Value::Integer(1), Value::Text(String::from("old"))],
    );
    with_validated_row_encoder(layout, |encoder| {
        let payload_measurement = payload_row
            .measure_direct_update(&payload_update, &encoder)
            .expect("payload update measures");
        let encoded_len = payload_measurement.encoded_len();
        let mut payload_budget =
            ByteBudget::new(installed_bytes - 1, Resource::StorageWorkingBytes);
        assert!(matches!(
            payload_row.install_direct_update(&payload_update, &mut payload_budget),
            Err(Error::ResourceLimit {
                resource: Resource::StorageWorkingBytes,
                limit,
            }) if limit == installed_bytes - 1
        ));
        assert_eq!(payload_budget.used, 0);

        let mut retry_budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);
        payload_row
            .install_direct_update(&payload_update, &mut retry_budget)
            .expect("payload update remains installable");
        payload_row
            .encode_effective_update(&encoder, payload_measurement, &mut retry_budget)
            .expect("retried payload update encodes");
        assert_eq!(retry_budget.used, encoded_len);
    });

    let mut clone_assignments = vec![(0, Value::Integer(2)), (1, Value::Text(long_text))];
    let clone_update =
        PreparedDirectUpdate::new(&mut clone_assignments, layout.column_count(), identity)
            .expect("clone update prepares");
    let mut clone_row = FrozenRow::new(
        identity,
        vec![Value::Integer(1), Value::Text(String::from("old"))],
    );
    with_validated_row_encoder(layout, |encoder| {
        let clone_measurement = clone_row
            .measure_direct_update(&clone_update, &encoder)
            .expect("clone update measures");
        let clone_encoded_len = clone_measurement.encoded_len();
        let mut clone_budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);
        set_value_clone_failure_after(Some(1));
        let clone_error = clone_row
            .install_direct_update(&clone_update, &mut clone_budget)
            .expect_err("the second cloned value fails");
        set_value_clone_failure_after(None);
        assert!(matches!(
            clone_error,
            Error::Allocation {
                operation: "cloning a direct mutation value"
            }
        ));
        assert_eq!(clone_budget.used, 0);

        clone_row
            .install_direct_update(&clone_update, &mut clone_budget)
            .expect("clone failure leaves the row installable");
        clone_row
            .encode_effective_update(&encoder, clone_measurement, &mut clone_budget)
            .expect("retried clone update encodes");
        assert_eq!(clone_budget.used, clone_encoded_len);
    });
}

#[test]
fn deferred_sequence_peak_excludes_consumed_measurements_and_overlays() {
    fn plan_row_then_sequence(
        limit: usize,
        descriptor: usize,
        sequence_bytes: usize,
        table: &str,
        columns: &[SchemaColumn],
        original_record: &str,
    ) -> crate::Result<(usize, usize)> {
        let layout = validate_row_layout(RowLayout { table, columns })?;
        let identity = RowIdentity::new(0..original_record.len())?;
        let mut assignments = vec![(1, Value::Text(String::new()))];
        let update = PreparedDirectUpdate::new(&mut assignments, layout.column_count(), identity)?;
        let mut budget = ByteBudget::new(limit, Resource::StorageWorkingBytes);
        budget.charge(descriptor)?;
        let mut rows = Vec::new();
        let _ = budget.reserve_exact(&mut rows, 1, "reserving a sequence test target")?;
        let decoded = decoded_values_bytes(columns.len(), original_record.len(), &budget)?;
        budget.charge(decoded)?;
        rows.push(FrozenRow::new(
            identity,
            vec![Value::Integer(1), Value::Text(String::from("one"))],
        ));

        with_validated_row_encoder(layout, |encoder| {
            let (measurements, measurement_working_bytes) = measure_and_check_update_database_size(
                original_record.len(),
                usize::MAX,
                &mut rows,
                &encoder,
                &update,
                None,
                &mut budget,
            )?;
            let encoded_row_bytes = measurements[0].encoded_len();
            rows[0].install_direct_update(&update, &mut budget)?;
            let row_encoding_peak = budget
                .used
                .checked_add(encoded_row_bytes)
                .expect("the test peak fits");
            rows[0].encode_effective_update(
                &encoder,
                measurements
                    .into_iter()
                    .next()
                    .expect("one measurement exists"),
                &mut budget,
            )?;
            budget.release(measurement_working_bytes);
            let retained_after_row = budget.used;
            budget.check_transient(sequence_bytes)?;
            Ok((retained_after_row, row_encoding_peak))
        })
    }

    let table = "t".repeat(256);
    let original_record = format!("~R|{table}|I1|Tone;");
    let blob =
        format!("V2;~S|{table}|id:I:!|body:T:!;~P|{table}|id;~A|{table}|id|I1;{original_record}");
    let state = StorageState::load(blob, usize::MAX).expect("sequence fixture loads");
    let mut candidate = state.candidate(state.as_str().len()).expect("source fits");
    candidate
        .defer_auto_increment(&table, 10)
        .expect("sequence can advance");
    let descriptor = candidate.deferred_auto_increment_working_bytes();
    let sequence_bytes = candidate
        .deferred_auto_increment_lengths()
        .expect("sequence measures")
        .expect("sequence edit exists")
        .1;
    let columns = vec![
        SchemaColumn {
            name: String::from("id"),
            data_type: DataType::Integer,
            nullable: false,
            default: None,
        },
        SchemaColumn {
            name: String::from("body"),
            data_type: DataType::Text,
            nullable: false,
            default: None,
        },
    ];

    let (retained_after_row, row_peak) = plan_row_then_sequence(
        usize::MAX,
        descriptor,
        sequence_bytes,
        &table,
        &columns,
        &original_record,
    )
    .expect("the row and sequence peaks are measurable");
    let exact = retained_after_row + sequence_bytes;
    assert!(
        exact > row_peak,
        "the long table name makes sequence encoding the final peak"
    );
    assert_eq!(
        plan_row_then_sequence(
            exact,
            descriptor,
            sequence_bytes,
            &table,
            &columns,
            &original_record,
        )
        .expect("the exact sequence peak fits")
        .0,
        retained_after_row
    );
    assert!(matches!(
        plan_row_then_sequence(
            exact - 1,
            descriptor,
            sequence_bytes,
            &table,
            &columns,
            &original_record,
        ),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit,
        }) if limit == exact - 1
    ));
}

#[test]
fn deferred_sequence_reservation_is_charged_before_candidate_allocation() {
    let blob = String::from("V2;~S|t|id:I:!;~P|t|id;~A|t|id|I1;~R|t|I1;");
    let state = StorageState::load(blob, usize::MAX).expect("sequence fixture loads");
    let mut candidate = state.candidate(state.as_str().len()).expect("source fits");
    let reservation = candidate
        .deferred_auto_increment_reservation_bytes()
        .expect("the deferred index can be measured");
    let mut budget = ByteBudget::new(reservation - 1, Resource::StorageWorkingBytes);

    assert!(matches!(
        defer_auto_increment(&mut candidate, "t", 10, &mut budget),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit,
        }) if limit == reservation - 1
    ));
    assert_eq!(budget.used, 0);
    assert_eq!(candidate.deferred_auto_increment_working_bytes(), 0);
    assert_eq!(
        candidate
            .deferred_auto_increment_lengths()
            .expect("the failed reservation leaves no edit"),
        None
    );
}

#[test]
fn governed_reservations_report_allocation_failures() {
    let mut budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);
    budget.charge(7).expect("baseline fits");
    let mut values = Vec::<u8>::new();
    let impossible = (isize::MAX as usize).saturating_add(1);

    assert!(matches!(
        budget.reserve_exact(
            &mut values,
            impossible,
            "forcing a test reservation failure"
        ),
        Err(Error::Allocation {
            operation: "forcing a test reservation failure"
        })
    ));
    assert!(values.is_empty());
    assert_eq!(budget.used, 7, "failed reservations refund their charge");
}

#[test]
fn update_restrict_releases_only_edges_the_same_statement_retargets() {
    let record = "~R|nodes|I3|I3;";
    let blob = format!(
        "V2;~S|nodes|id:I:!|parent_id:I:?;~P|nodes|id;~F|nodes|parent_id|nodes|id;{record}"
    );
    let start = blob.find(record).expect("the fixture holds one row record");
    let columns = vec![
        SchemaColumn {
            name: String::from("id"),
            data_type: DataType::Integer,
            nullable: false,
            default: None,
        },
        SchemaColumn {
            name: String::from("parent_id"),
            data_type: DataType::Integer,
            nullable: true,
            default: None,
        },
    ];
    let state = StorageState::load(blob, usize::MAX).expect("the self-reference loads");
    let schema = state
        .catalog()
        .table("nodes")
        .expect("the fixture declares nodes");

    let primary_key = schema.primary_key.expect("nodes declares a primary key");
    let enforce = |assignments: &mut [(usize, Value)]| -> crate::Result<()> {
        let layout = RowLayout {
            table: "nodes",
            columns: &columns,
        };
        let mut budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);
        let (mut rows, _) = freeze_rows(
            state.as_str(),
            [Ok(start..start + record.len())],
            layout,
            &mut budget,
            |_| Ok(true),
        )
        .expect("the validated record freezes");
        let update = PreparedDirectUpdate::new(assignments, columns.len(), rows[0].identity())?;
        for row in &mut rows {
            row.install_direct_update(&update, &mut budget)?;
        }

        let mut update_queue = Vec::new();
        let mut queue_working_bytes = 0;
        for (index, row) in rows.iter_mut().enumerate() {
            if row.mark_update_queued(primary_key) {
                push_update_queue(
                    &mut update_queue,
                    index,
                    &mut queue_working_bytes,
                    &mut budget,
                )?;
            }
        }
        if update_queue.is_empty() {
            return Ok(());
        }
        let referential = ReferentialIndex::build(
            state.as_str(),
            state.catalog(),
            "nodes",
            &rows,
            ReferentialAction::Update,
            &mut budget,
        )?;
        referential.initialize_direct_rows(&rows)?;
        referential.expand_update_actions(
            &mut rows,
            &mut update_queue,
            &mut queue_working_bytes,
            &mut budget,
        )
    };

    // Re-keying alone leaves the row's own reference behind on the old key.
    assert!(matches!(
        enforce(&mut [(0, Value::Integer(4))]),
        Err(Error::Constraint(ref message))
            if message == "foreign key \"nodes\".\"parent_id\" restricts mutation of \"nodes\""
    ));
    // Assigning the foreign-key column something else that still names the old
    // key restricts for the same reason.
    assert!(matches!(
        enforce(&mut [(0, Value::Integer(4)), (1, Value::Integer(3))]),
        Err(Error::Constraint(_))
    ));
    // Moving the reference with the key releases the edge; whether the new key
    // resolves is left to candidate validation.
    enforce(&mut [(0, Value::Integer(4)), (1, Value::Integer(4))])
        .expect("a co-mutated child does not restrict its own parent");
    enforce(&mut [(0, Value::Integer(4)), (1, Value::Null)])
        .expect("a nulled reference does not restrict its parent");
}
