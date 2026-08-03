use super::partition;
use crate::Value;
use crate::expression::{LikeAtom, Predicate, Program, ProgramNode};
use crate::resolve::ColumnLocation;

const fn location(source: usize, column: usize) -> ColumnLocation {
    ColumnLocation { source, column }
}

#[test]
fn top_level_factors_split_into_regex_local_and_cross_source_programs() {
    let pushed = Value::Integer(1);
    let left = Value::Integer(2);
    let right = Value::Integer(3);
    let program = Program::new(vec![
        ProgramNode::And { children: 3 },
        ProgramNode::Predicate(Predicate::Equal {
            column: location(0, 0),
            value: &pushed,
        }),
        ProgramNode::Or { children: 2 },
        ProgramNode::Predicate(Predicate::IsNull {
            column: location(1, 1),
        }),
        ProgramNode::Predicate(Predicate::IsNotNull {
            column: location(1, 2),
        }),
        ProgramNode::Or { children: 2 },
        ProgramNode::Predicate(Predicate::Equal {
            column: location(0, 0),
            value: &left,
        }),
        ProgramNode::Predicate(Predicate::NotEqual {
            column: location(1, 0),
            value: &right,
        }),
    ]);

    let partition = partition(Some(program), 2).expect("expression partitions");

    assert!(matches!(
        partition.regex_by_source[0].as_slice(),
        [Predicate::Equal { column, value }]
            if *column == location(0, 0) && **value == Value::Integer(1)
    ));
    assert!(partition.regex_by_source[1].is_empty());
    assert!(partition.local_residuals[0].is_none());
    let local = partition.local_residuals[1]
        .as_ref()
        .expect("source one has a local residual");
    assert!(matches!(local.nodes()[0], ProgramNode::Or { children: 2 }));
    assert!(matches!(
        local.nodes()[1],
        ProgramNode::Predicate(Predicate::IsNull { column })
            if column == location(1, 1)
    ));
    assert!(matches!(
        local.nodes()[2],
        ProgramNode::Predicate(Predicate::IsNotNull { column })
            if column == location(1, 2)
    ));

    let cross = partition
        .cross_source_residual
        .as_ref()
        .expect("mixed-source factor remains cross-source");
    assert!(matches!(cross.nodes()[0], ProgramNode::Or { children: 2 }));
    assert!(matches!(
        cross.nodes()[1],
        ProgramNode::Predicate(Predicate::Equal { column, .. })
            if column == location(0, 0)
    ));
    assert!(matches!(
        cross.nodes()[2],
        ProgramNode::Predicate(Predicate::NotEqual { column, .. })
            if column == location(1, 0)
    ));
}

#[test]
fn predicates_beneath_or_are_never_regex_pushed() {
    let value = Value::Text(String::from("kept"));
    let program = Program::new(vec![
        ProgramNode::Or { children: 2 },
        ProgramNode::Predicate(Predicate::Equal {
            column: location(0, 0),
            value: &value,
        }),
        ProgramNode::Predicate(Predicate::Like {
            column: location(0, 0),
            atoms: vec![LikeAtom::Literal('k'), LikeAtom::AnySequence],
        }),
    ]);

    let partition = partition(Some(program), 1).expect("expression partitions");

    assert!(partition.regex_by_source[0].is_empty());
    assert!(partition.cross_source_residual.is_none());
    assert!(matches!(
        partition.local_residuals[0]
            .as_ref()
            .expect("OR remains a residual")
            .nodes()[0],
        ProgramNode::Or { children: 2 }
    ));
}

#[test]
fn partition_moves_like_atom_buffers_into_their_destinations() {
    let pushed = Program::new(vec![ProgramNode::Predicate(Predicate::Like {
        column: location(0, 0),
        atoms: vec![LikeAtom::Literal('p'), LikeAtom::AnySequence],
    })]);
    let pushed_atoms = match &pushed.nodes()[0] {
        ProgramNode::Predicate(Predicate::Like { atoms, .. }) => atoms.as_ptr(),
        other => panic!("expected LIKE predicate, got {other:?}"),
    };
    let pushed_partition = partition(Some(pushed), 1).expect("LIKE predicate partitions");
    let Predicate::Like { atoms, .. } = &pushed_partition.regex_by_source[0][0] else {
        panic!("expected pushed LIKE predicate");
    };
    assert_eq!(atoms.as_ptr(), pushed_atoms);

    let residual = Program::new(vec![
        ProgramNode::Or { children: 2 },
        ProgramNode::Predicate(Predicate::Like {
            column: location(0, 0),
            atoms: vec![LikeAtom::Literal('r'), LikeAtom::AnyScalar],
        }),
        ProgramNode::Predicate(Predicate::IsNull {
            column: location(0, 0),
        }),
    ]);
    let residual_atoms = match &residual.nodes()[1] {
        ProgramNode::Predicate(Predicate::Like { atoms, .. }) => atoms.as_ptr(),
        other => panic!("expected LIKE predicate, got {other:?}"),
    };
    let residual_partition = partition(Some(residual), 1).expect("OR factor partitions");
    let residual = residual_partition.local_residuals[0]
        .as_ref()
        .expect("OR remains residual");
    let ProgramNode::Predicate(Predicate::Like { atoms, .. }) = &residual.nodes()[1] else {
        panic!("expected residual LIKE predicate");
    };
    assert_eq!(atoms.as_ptr(), residual_atoms);
}

#[test]
fn safe_not_equal_and_like_leaves_are_pushed_without_residuals() {
    let value = Value::Text(String::from("skip"));
    let program = Program::new(vec![
        ProgramNode::And { children: 2 },
        ProgramNode::Predicate(Predicate::NotEqual {
            column: location(0, 0),
            value: &value,
        }),
        ProgramNode::Predicate(Predicate::Like {
            column: location(0, 1),
            atoms: vec![LikeAtom::AnySequence],
        }),
    ]);

    let partition = partition(Some(program), 1).expect("expression partitions");

    assert!(matches!(
        partition.regex_by_source[0].as_slice(),
        [Predicate::NotEqual { .. }, Predicate::Like { .. }]
    ));
    assert!(partition.local_residuals[0].is_none());
    assert!(partition.cross_source_residual.is_none());
}
