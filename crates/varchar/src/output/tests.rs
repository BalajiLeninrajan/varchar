use super::{ColumnOrigin, ResultColumn, RowSet, SelectExplanation};
use crate::{DataType, Value};

#[test]
fn snapshots_expose_read_and_consuming_views() {
    let origin = ColumnOrigin::new(String::from("items"), String::from("id"));
    let column = ResultColumn::new(String::from("id"), origin, DataType::Integer, false);
    let row_set = RowSet::new(vec![column.clone()], vec![vec![Value::Integer(1)]]);
    let explanation = SelectExplanation::new(
        String::from("row-pattern"),
        false,
        vec![String::from("items")],
        vec![column.clone()],
    );
    let exact = SelectExplanation::new(
        String::from("row-pattern"),
        true,
        vec![String::from("items")],
        vec![column],
    );

    assert_eq!(row_set.columns()[0].label(), "id");
    assert_eq!(row_set.columns()[0].origin().table(), "items");
    assert_eq!(row_set.columns()[0].origin().column(), "id");
    assert_eq!(row_set.columns()[0].data_type(), DataType::Integer);
    assert!(!row_set.columns()[0].nullable());
    assert_eq!(row_set.rows(), &[vec![Value::Integer(1)]]);
    assert_eq!(row_set.clone().into_rows(), vec![vec![Value::Integer(1)]]);
    let (columns, rows) = row_set.into_parts();
    assert_eq!(columns.len(), 1);
    assert_eq!(rows, vec![vec![Value::Integer(1)]]);

    assert_eq!(explanation.pattern(), "row-pattern");
    assert!(!explanation.pattern_is_exact());
    assert!(exact.pattern_is_exact());
    assert_ne!(explanation, exact);
    assert_eq!(explanation.sources(), &[String::from("items")]);
    assert_eq!(explanation.columns()[0].label(), "id");
}

#[test]
#[cfg(debug_assertions)]
#[should_panic]
fn row_set_rejects_inconsistent_row_widths_in_debug_builds() {
    RowSet::new(
        vec![ResultColumn::new(
            String::from("id"),
            ColumnOrigin::new(String::from("items"), String::from("id")),
            DataType::Integer,
            false,
        )],
        vec![vec![Value::Integer(1), Value::Integer(2)]],
    );
}
