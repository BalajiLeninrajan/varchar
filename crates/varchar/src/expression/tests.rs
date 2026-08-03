use super::like;
use super::truth::Truth;
use super::{
    CheckPredicate, CheckProgramNode, Evaluator, LikeAtom, Predicate, Program, ProgramNode,
    compile_pattern, is_well_formed,
};
use crate::resolve::ColumnLocation;
use crate::sql::{ColumnRef, ExpressionNode, Predicate as ParsedPredicate, PredicateOperator};
use crate::{Error, Resource, Value};

fn where_leaf() -> ProgramNode<'static> {
    ProgramNode::Predicate(Predicate::IsNull {
        column: ColumnLocation {
            source: 0,
            column: 0,
        },
    })
}

fn passes_where(predicate: Predicate<'_>, row: &[Value]) -> bool {
    let program = Program::new(vec![ProgramNode::Predicate(predicate)]);
    let mut evaluator = Evaluator::new(&program, usize::MAX).expect("evaluation stack reserves");
    evaluator
        .evaluate_where(&program, &[row])
        .expect("predicate evaluates")
}

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
fn three_valued_or_matrix_is_complete() {
    use Truth::{False, True, Unknown};
    let cases = [
        (False, False, False),
        (False, True, True),
        (False, Unknown, Unknown),
        (True, False, True),
        (True, True, True),
        (True, Unknown, True),
        (Unknown, False, Unknown),
        (Unknown, True, True),
        (Unknown, Unknown, Unknown),
    ];
    for (left, right, expected) in cases {
        assert_eq!(left.or(right), expected, "{left:?} OR {right:?}");
    }
}

/// Match `pattern` against `value` with a private budget of `limit`.
fn like_matches(value: &str, pattern: &str, limit: usize) -> crate::Result<bool> {
    let atoms = compile_pattern(pattern).expect("LIKE pattern is valid");
    like::matches_charged(value, &atoms, &mut like::LikeWork::new(limit))
}

/// Exhaustive backtracking reference for `LIKE`, used to pin the fast matcher.
fn reference_matches(value: &[char], atoms: &[LikeAtom]) -> bool {
    match atoms.split_first() {
        None => value.is_empty(),
        Some((LikeAtom::AnySequence, rest)) => {
            (0..=value.len()).any(|skipped| reference_matches(&value[skipped..], rest))
        }
        Some((LikeAtom::AnyScalar, rest)) => {
            !value.is_empty() && reference_matches(&value[1..], rest)
        }
        Some((LikeAtom::Literal(expected), rest)) => {
            value.first() == Some(expected) && reference_matches(&value[1..], rest)
        }
    }
}

fn shorter_than(alphabet: &[char], length: usize) -> Vec<String> {
    let mut words = vec![String::new()];
    let mut frontier = vec![String::new()];
    for _ in 0..length {
        let mut next = Vec::new();
        for word in &frontier {
            for symbol in alphabet {
                let mut extended = word.clone();
                extended.push(*symbol);
                next.push(extended);
            }
        }
        words.extend(next.iter().cloned());
        frontier = next;
    }
    words
}

#[test]
fn decoded_like_matching_handles_unicode_and_validated_escapes() {
    let cases = [
        ("", "%", true),
        ("é", "_", true),
        ("éé", "_", false),
        ("aβz", "a%_", true),
        ("aβ", "a_", true),
        ("aβ", "a__", false),
        ("100%", r"100\%", true),
        ("a_b", r"a\_b", true),
        (r"a\b", r"a\\b", true),
        ("Ab", "a%", false),
        ("a💾b", "a%%__b", false),
        ("aXXb", "a%%__b", true),
        ("aéb", "%é%", true),
        ("abcabc", "%c%c", true),
        ("abcabc", "%c%c%", true),
        ("abcabc", "a%b%c", true),
        ("abcabc", "%bc%bc%", true),
        ("abcabc", "%bcb%", false),
    ];
    for (value, pattern, expected) in cases {
        assert_eq!(
            like_matches(value, pattern, usize::MAX).expect("a generous budget is never exhausted"),
            expected,
            "value {value:?}, pattern {pattern:?}"
        );
    }

    assert!(compile_pattern(r"bad\q").is_err());
    assert!(compile_pattern("trailing\\").is_err());
    assert_eq!(
        compile_pattern(r"\%\_\\").expect("escaped wildcard pattern resolves"),
        vec![
            LikeAtom::Literal('%'),
            LikeAtom::Literal('_'),
            LikeAtom::Literal('\\'),
        ]
    );
}

#[test]
fn segment_matching_agrees_with_exhaustive_backtracking() {
    // The matcher anchors the leading and trailing segments and places interior
    // ones greedily; every pattern shape over a two-symbol alphabet is compared
    // against a reference that tries every wildcard split instead.
    let values = shorter_than(&['a', 'b'], 5);
    let patterns = shorter_than(&['a', 'b', '%', '_'], 4);

    for pattern in &patterns {
        let atoms = compile_pattern(pattern).expect("wildcard pattern resolves");
        for value in &values {
            let scalars: Vec<char> = value.chars().collect();
            assert_eq!(
                like_matches(value, pattern, usize::MAX).expect("a generous budget is enough"),
                reference_matches(&scalars, &atoms),
                "value {value:?}, pattern {pattern:?}"
            );
        }
    }
}

#[test]
fn anchored_like_segments_do_not_rescan_the_value() {
    // A pattern whose only wildcard leads it is anchored at the end of the
    // value, so it costs the pattern rather than the product of both lengths.
    // This is the shape a pushed-down `LIKE` handles linearly; charging it
    // would refuse queries the scan pattern answers in microseconds.
    let value = "a".repeat(60_000);
    for run in [12_usize, 16, 20, 24, 30_000] {
        let pattern = format!("%{}b", "a".repeat(run));
        assert!(
            !like_matches(&value, &pattern, 1_000_000).expect("an anchored suffix is not charged"),
            "run {run} was refused"
        );
    }

    // A leading anchor and an interior literal are scanned forward once each.
    assert!(!like_matches(&value, "b%", 1_000_000).expect("a leading anchor is not charged"));
    assert!(!like_matches(&value, "%b%", 1_000_000).expect("an interior scan is not charged"));
}

#[test]
fn decoded_like_matching_charges_the_regex_backtracking_budget() {
    // An interior segment can still be retried at every scalar, so a repetitive
    // value against a long literal run does work proportional to the product of
    // the two lengths and must be refused rather than run to completion.
    let value = "a".repeat(4_096);

    assert!(matches!(
        like_matches(&value, "%aaaaaaaaaab%", 16),
        Err(Error::ResourceLimit {
            resource: Resource::RegexBacktracking,
            limit: 16,
        })
    ));
    assert!(
        !like_matches(&value, "%aaaaaaaaaab%", usize::MAX)
            .expect("a generous budget is never exhausted")
    );

    // The default budget refuses the adversarial shape at realistic sizes.
    let adversarial = format!("%{}b%", "a".repeat(30_000));
    assert!(matches!(
        like_matches(&value, &adversarial, 1_000_000),
        Err(Error::ResourceLimit {
            resource: Resource::RegexBacktracking,
            limit: 1_000_000,
        })
    ));
}

#[test]
fn one_budget_is_shared_by_every_like_search() {
    // Each search alone stays inside the budget; the budget is spent once for
    // the statement, so repeating the search must exhaust it rather than hand
    // out a fresh allowance per row or per predicate.
    let value = "a".repeat(256);
    let atoms = compile_pattern("%aaaaaaaaaab%").expect("interior pattern resolves");
    const BUDGET: usize = 4_000;

    let mut work = like::LikeWork::new(BUDGET);
    let mut searches = 0_usize;
    loop {
        match like::matches_charged(&value, &atoms, &mut work) {
            Ok(matched) => {
                assert!(!matched);
                searches += 1;
            }
            Err(Error::ResourceLimit {
                resource: Resource::RegexBacktracking,
                limit: BUDGET,
            }) => break,
            other => panic!("unexpected LIKE outcome: {other:?}"),
        }
        assert!(searches < 1_000, "the shared budget was never exhausted");
    }
    assert!(searches > 0, "one search alone must fit inside the budget");
}

#[test]
fn residual_like_evaluation_surfaces_the_regex_backtracking_limit() {
    let row = vec![Value::Text("a".repeat(4_096))];
    let rows = [row.as_slice()];
    let program = Program::new(vec![ProgramNode::Predicate(Predicate::Like {
        column: ColumnLocation {
            source: 0,
            column: 0,
        },
        atoms: compile_pattern("%aaaaaaaaaab%").expect("LIKE resolves"),
    })]);

    let mut bounded = Evaluator::new(&program, 16).expect("evaluation stack reserves");
    assert!(matches!(
        bounded.evaluate_where(&program, &rows),
        Err(Error::ResourceLimit {
            resource: Resource::RegexBacktracking,
            limit: 16,
        })
    ));

    let mut generous = Evaluator::new(&program, usize::MAX).expect("evaluation stack reserves");
    assert!(
        !generous
            .evaluate_where(&program, &rows)
            .expect("a generous budget evaluates the residual LIKE")
    );

    // The evaluator holds one budget for its whole scan. A row the evaluator
    // accepts once must still exhaust the budget when the scan keeps visiting
    // rows, otherwise the bound would be multiplied by the row count.
    const SCAN_BUDGET: usize = 100_000;
    let mut scanning = Evaluator::new(&program, SCAN_BUDGET).expect("evaluation stack reserves");
    let mut visited = 0_usize;
    loop {
        match scanning.evaluate_where(&program, &rows) {
            Ok(matched) => {
                assert!(!matched);
                visited += 1;
            }
            Err(Error::ResourceLimit {
                resource: Resource::RegexBacktracking,
                limit: SCAN_BUDGET,
            }) => break,
            other => panic!("unexpected residual LIKE outcome: {other:?}"),
        }
        assert!(visited < 10_000, "a scan never exhausted its shared budget");
    }
    assert!(visited > 0, "one row alone must fit inside the budget");
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
    let mut evaluator = Evaluator::new(&equal_null, usize::MAX).expect("evaluation stack reserves");
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
    let mut evaluator = Evaluator::new(&is_null, usize::MAX).expect("evaluation stack reserves");
    assert!(
        evaluator
            .evaluate_where(&is_null, &rows)
            .expect("IS NULL evaluates")
    );

    let like = Program::new(vec![ProgramNode::Predicate(Predicate::Like {
        column: ColumnLocation {
            source: 0,
            column: 1,
        },
        atoms: compile_pattern("x%").expect("LIKE resolves"),
    })]);
    let mut evaluator = Evaluator::new(&like, usize::MAX).expect("evaluation stack reserves");
    assert!(
        evaluator
            .evaluate_where(&like, &rows)
            .expect("decoded LIKE evaluates")
    );
}

#[test]
fn ordered_predicates_compare_each_scalar_type_and_reject_null_left_values() {
    let location = ColumnLocation {
        source: 0,
        column: 0,
    };
    let integer = Value::Integer(2);
    assert!(passes_where(
        Predicate::LessThan {
            column: location,
            value: &integer,
        },
        &[Value::Integer(1)],
    ));
    assert!(passes_where(
        Predicate::LessThanOrEqual {
            column: location,
            value: &integer,
        },
        &[Value::Integer(2)],
    ));
    assert!(passes_where(
        Predicate::GreaterThan {
            column: location,
            value: &integer,
        },
        &[Value::Integer(3)],
    ));
    assert!(passes_where(
        Predicate::GreaterThanOrEqual {
            column: location,
            value: &integer,
        },
        &[Value::Integer(2)],
    ));

    let text = Value::Text(String::from("β"));
    assert!(passes_where(
        Predicate::LessThan {
            column: location,
            value: &text,
        },
        &[Value::Text(String::from("é"))],
    ));
    let boolean = Value::Boolean(true);
    assert!(passes_where(
        Predicate::LessThan {
            column: location,
            value: &boolean,
        },
        &[Value::Boolean(false)],
    ));
    assert!(!passes_where(
        Predicate::LessThan {
            column: location,
            value: &integer,
        },
        &[Value::Null],
    ));
}

#[test]
fn in_membership_honors_matches_nulls_and_duplicates() {
    let location = ColumnLocation {
        source: 0,
        column: 0,
    };
    let with_null = vec![Value::Integer(1), Value::Null, Value::Integer(1)];
    assert!(passes_where(
        Predicate::In {
            column: location,
            values: &with_null,
        },
        &[Value::Integer(1)],
    ));
    assert!(!passes_where(
        Predicate::In {
            column: location,
            values: &with_null,
        },
        &[Value::Integer(2)],
    ));
    assert!(!passes_where(
        Predicate::In {
            column: location,
            values: &with_null,
        },
        &[Value::Null],
    ));

    let without_null = vec![Value::Integer(1), Value::Integer(1)];
    assert!(!passes_where(
        Predicate::In {
            column: location,
            values: &without_null,
        },
        &[Value::Integer(2)],
    ));
}

/// One walk backs every pipeline, so each node type rejects the same shapes.
#[test]
fn every_pipeline_rejects_the_same_malformed_programs() {
    let parsed_empty_in = vec![ExpressionNode::Predicate(ParsedPredicate {
        column: ColumnRef {
            qualifier: None,
            name: String::from("value"),
        },
        operator: PredicateOperator::In(Vec::new()),
    })];
    assert!(!is_well_formed(&parsed_empty_in));

    let resolved_empty_in = vec![ProgramNode::Predicate(Predicate::In {
        column: ColumnLocation {
            source: 0,
            column: 0,
        },
        values: &[],
    })];
    assert!(!is_well_formed(&resolved_empty_in));

    let check_empty_in = vec![CheckProgramNode::Predicate(CheckPredicate::In {
        column: 0,
        values: Vec::new(),
    })];
    assert!(!is_well_formed(&check_empty_in));

    assert!(
        !is_well_formed(&[] as &[ProgramNode<'_>]),
        "an empty program has no root"
    );
    assert!(
        !is_well_formed(&[ProgramNode::And { children: 1 }, where_leaf()]),
        "a logical node needs at least two children"
    );
    assert!(
        !is_well_formed(&[ProgramNode::And { children: 2 }, where_leaf()]),
        "a program cannot end before all children are encoded"
    );
    assert!(
        !is_well_formed(&[where_leaf(), where_leaf()]),
        "a program cannot carry a second root"
    );
}
