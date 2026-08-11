use fancy_regex::Regex;

use super::{alternate_source_patterns, row_scan_pattern};
use crate::resolve::{ColumnLocation, LikeAtom, ResolvedPredicate};
use crate::{DataType, SchemaColumn, Value};

const MAX_PATTERN_BYTES: usize = 16 * 1024;

fn matches(pattern: &str, row: &str) -> bool {
    Regex::new(&format!("^(?:{pattern})$"))
        .expect("test pattern compiles")
        .is_match(row)
        .expect("test pattern executes")
}

const fn location(column: usize) -> ColumnLocation {
    ColumnLocation { source: 0, column }
}

fn integer_column(name: &str) -> SchemaColumn {
    SchemaColumn {
        name: name.to_owned(),
        data_type: DataType::Integer,
        nullable: false,
    }
}

#[test]
fn complete_patterns_route_exact_tables_and_row_boundaries() {
    let columns = [integer_column("id")];
    let pattern = row_scan_pattern("user", &columns, &[], MAX_PATTERN_BYTES).expect("row pattern");

    assert!(matches(&pattern, "~R|user|I1;"));
    assert!(!matches(&pattern, "~R|users|I1;"));
    assert!(!matches(&pattern, "~R|user|I1;~R|user|I2;"));
}

#[test]
fn resolved_like_atoms_match_canonical_encoded_text() {
    let columns = [SchemaColumn {
        name: String::from("body"),
        data_type: DataType::Text,
        nullable: true,
    }];
    let predicates = [ResolvedPredicate::Like {
        column: location(0),
        atoms: vec![
            LikeAtom::Literal('|'),
            LikeAtom::AnySequence,
            LikeAtom::Literal(';'),
        ],
    }];
    let pattern =
        row_scan_pattern("notes", &columns, &predicates, MAX_PATTERN_BYTES).expect("row pattern");

    assert!(matches(&pattern, "~R|notes|T%00007Cmiddle%00003B;"));
    assert!(!matches(&pattern, "~R|notes|Tmiddle;"));
    assert!(!matches(&pattern, "~R|notes|N;"));
}

#[test]
fn null_and_typed_value_predicates_use_complete_cell_boundaries() {
    let columns = [
        integer_column("id"),
        SchemaColumn {
            name: String::from("note"),
            data_type: DataType::Text,
            nullable: true,
        },
    ];
    let id = Value::Integer(7);
    let predicates = [
        ResolvedPredicate::Equal {
            column: location(0),
            value: &id,
        },
        ResolvedPredicate::IsNotNull {
            column: location(1),
        },
    ];
    let value_pattern =
        row_scan_pattern("items", &columns, &predicates, MAX_PATTERN_BYTES).expect("value pattern");
    let null_pattern = row_scan_pattern(
        "items",
        &columns,
        &[ResolvedPredicate::IsNull {
            column: location(1),
        }],
        MAX_PATTERN_BYTES,
    )
    .expect("NULL pattern");

    assert!(matches(&value_pattern, "~R|items|I7|Tkept;"));
    assert!(!matches(&value_pattern, "~R|items|I70|Tkept;"));
    assert!(!matches(&value_pattern, "~R|items|I7|N;"));
    assert!(matches(&null_pattern, "~R|items|I70|N;"));
    assert!(!matches(&null_pattern, "~R|items|I70|Tvalue;"));
}

#[test]
fn alternation_matches_each_join_source_only() {
    let columns = [integer_column("id")];
    let pattern = alternate_source_patterns(
        [
            row_scan_pattern("users", &columns, &[], MAX_PATTERN_BYTES),
            row_scan_pattern("teams", &columns, &[], MAX_PATTERN_BYTES),
        ],
        MAX_PATTERN_BYTES,
    )
    .expect("source alternation");

    assert!(matches(&pattern, "~R|users|I1;"));
    assert!(matches(&pattern, "~R|teams|I2;"));
    assert!(!matches(&pattern, "~R|roles|I3;"));
}

#[test]
fn invalid_predicate_column_indices_return_an_error() {
    let columns = [integer_column("id")];
    let error = row_scan_pattern(
        "items",
        &columns,
        &[ResolvedPredicate::IsNull {
            column: location(1),
        }],
        MAX_PATTERN_BYTES,
    )
    .expect_err("an out-of-range predicate is rejected");

    assert!(matches!(error, crate::Error::Schema(_)));
}
