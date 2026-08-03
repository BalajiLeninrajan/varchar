use super::model::{FrozenRow, RowIdentity, WorkingBudget};
use super::{freeze_rows, sort_and_validate_ranges};
use crate::storage::RowLayout;
use crate::{DataType, Error, SchemaColumn, Value};

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
    let mut budget = WorkingBudget::with_limit(usize::MAX);
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
fn governed_reservations_report_allocation_failures() {
    let mut budget = WorkingBudget::with_limit(usize::MAX);
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
    assert_eq!(budget.used(), 7, "failed reservations refund their charge");
}
