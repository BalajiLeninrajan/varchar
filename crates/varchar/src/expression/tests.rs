use super::truth::Truth;
use super::{Evaluator, Predicate, Program, ProgramNode};
use crate::Value;
use crate::resolve::ColumnLocation;

#[test]
fn three_valued_and_matrix_is_complete() {
    use Truth::{False, True, Unknown};
    let cases = [
        (False, False, False),
        (False, True, False),
        (False, Unknown, False),
        (True, False, False),
        (True, True, True),
        (True, Unknown, Unknown),
        (Unknown, False, False),
        (Unknown, True, Unknown),
        (Unknown, Unknown, Unknown),
    ];
    for (left, right, expected) in cases {
        assert_eq!(left.and(right), expected, "{left:?} AND {right:?}");
    }
}

#[test]
fn existing_leaf_operators_use_sql_null_semantics() {
    let expected = Value::Text("x".to_owned());
    let row = vec![Value::Null, Value::Text("xyz".to_owned())];
    let rows = [row.as_slice()];

    let equal_null = Program::new(vec![ProgramNode::Predicate(Predicate::Equal {
        column: ColumnLocation {
            source: 0,
            column: 0,
        },
        value: &expected,
    })]);
    let mut evaluator = Evaluator::new(&equal_null).expect("evaluation stack reserves");
    assert!(
        !evaluator
            .evaluate_where(&equal_null, &rows)
            .expect("NULL equality evaluates")
    );

    let is_null = Program::new(vec![ProgramNode::Predicate(Predicate::IsNull {
        column: ColumnLocation {
            source: 0,
            column: 0,
        },
    })]);
    let mut evaluator = Evaluator::new(&is_null).expect("evaluation stack reserves");
    assert!(
        evaluator
            .evaluate_where(&is_null, &rows)
            .expect("IS NULL evaluates")
    );
}
