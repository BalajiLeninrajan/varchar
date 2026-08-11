use super::{DeferredAutoIncrement, StorageState};
use crate::expression::{CheckPredicate, CheckProgram, CheckProgramNode, LikeAtom};
use crate::storage::TableSchema;
use crate::storage::encode::measure_table_metadata;
use crate::storage::format::{FormatVersion, V2_HEADER, V3_HEADER};
use crate::{DataType, Error, Resource, SchemaColumn};

#[test]
fn failed_splice_leaves_the_candidate_reusable() {
    let state = StorageState::load(String::from("V2;~S|t|id:I:!;~R|t|I1;"), usize::MAX)
        .expect("source is valid");
    let source = state.as_str();
    let max_bytes = 256;
    let mut candidate = state.candidate(max_bytes).expect("source fits");
    let oversized = "x".repeat(max_bytes + 1);

    assert!(candidate.splice(3..source.len(), &oversized).is_err());
    assert_eq!(
        candidate
            .finish()
            .expect("unchanged candidate fits")
            .as_str(),
        source
    );
}

#[test]
fn deferred_sequence_edit_reports_working_and_replacement_bytes() {
    let state = StorageState::load(
        String::from("V2;~S|t|id:I:!;~P|t|id;~A|t|id|I1;~R|t|I1;"),
        usize::MAX,
    )
    .expect("source is valid");
    let mut candidate = state.candidate(state.as_str().len()).expect("source fits");

    assert_eq!(candidate.deferred_auto_increment_working_bytes(), 0);
    assert_eq!(
        candidate
            .deferred_auto_increment_lengths()
            .expect("an absent edit has no lengths"),
        None
    );

    candidate
        .defer_auto_increment("t", 10)
        .expect("the sequence can advance");
    assert_eq!(
        candidate.deferred_auto_increment_working_bytes(),
        std::mem::size_of::<DeferredAutoIncrement<'_>>()
    );
    assert_eq!(
        candidate
            .deferred_auto_increment_lengths()
            .expect("the deferred edit has exact lengths"),
        Some(("~A|t|id|I1;".len(), "~A|t|id|I10;".len()))
    );
    candidate.discard_deferred_auto_increment();
    assert_eq!(candidate.deferred_auto_increment_working_bytes(), 0);
    assert_eq!(
        candidate
            .finish()
            .expect("the untouched candidate validates")
            .as_str(),
        state.as_str()
    );
}

#[test]
fn finish_rejects_an_invalid_replacement_state() {
    let state = StorageState::empty();
    let mut candidate = state.candidate(64).expect("empty state fits");
    candidate
        .splice(state.as_str().len()..state.as_str().len(), "garbage")
        .expect("unvalidated edit fits");

    assert!(matches!(
        candidate.finish(),
        Err(crate::Error::CorruptStorage { .. })
    ));
    assert_eq!(state.as_str(), "V2;");
}

fn checked_text_schema(name: &str) -> TableSchema {
    TableSchema {
        name: String::from(name),
        columns: vec![SchemaColumn {
            name: String::from("value"),
            data_type: DataType::Text,
            nullable: true,
            default: None,
        }],
        primary_key: None,
        unique_columns: Vec::new(),
        foreign_keys: Vec::new(),
        checks: vec![CheckProgram::new(vec![CheckProgramNode::Predicate(
            CheckPredicate::Like {
                column: 0,
                atoms: vec![
                    LikeAtom::AnySequence,
                    LikeAtom::AnyScalar,
                    LikeAtom::Literal('%'),
                    LikeAtom::Literal('|'),
                    LikeAtom::Literal(';'),
                    LikeAtom::Literal('~'),
                    LikeAtom::Literal('\0'),
                    LikeAtom::Literal('\u{2028}'),
                    LikeAtom::Literal('\u{2029}'),
                    LikeAtom::Literal('é'),
                ],
            },
        )])],
    }
}

#[test]
fn table_metadata_create_checks_the_complete_boundary_before_v3_mutation() {
    let state = StorageState::empty();
    let schema = checked_text_schema("bounded");
    let measured = measure_table_metadata(&schema, None).expect("schema metadata measures");
    let exact = state
        .as_str()
        .len()
        .checked_sub(V2_HEADER.len())
        .and_then(|length| length.checked_add(V3_HEADER.len()))
        .and_then(|length| length.checked_add(measured.encoded_len()))
        .expect("fixture length fits");

    let mut exact_candidate = state.candidate(exact).expect("source fits exact limit");
    exact_candidate
        .insert_schema_with_auto_increment(&schema, None)
        .expect("exact projected database size succeeds");
    let exact_state = exact_candidate.finish().expect("exact candidate validates");
    assert_eq!(exact_state.as_str().len(), exact);
    assert!(exact_state.as_str().starts_with(V3_HEADER));

    let limit = exact - 1;
    let mut candidate = state.candidate(limit).expect("source fits lower limit");
    assert!(matches!(
        candidate.insert_schema_with_auto_increment(&schema, None),
        Err(Error::ResourceLimit {
            resource: Resource::DatabaseBytes,
            limit: error_limit,
        }) if error_limit == limit
    ));
    assert_eq!(candidate.cursor, 0);
    assert!(candidate.output.is_empty());
    assert_eq!(candidate.format, FormatVersion::V2);
    assert_eq!(state.as_str(), V2_HEADER);

    let fallback = TableSchema {
        name: String::from("small"),
        columns: vec![SchemaColumn {
            name: String::from("id"),
            data_type: DataType::Integer,
            nullable: true,
            default: None,
        }],
        primary_key: None,
        unique_columns: Vec::new(),
        foreign_keys: Vec::new(),
        checks: Vec::new(),
    };
    candidate
        .insert_schema_with_auto_increment(&fallback, None)
        .expect("candidate remains reusable after the rejected upgrade");
    assert_eq!(
        candidate
            .finish()
            .expect("fallback candidate validates")
            .as_str(),
        "V2;~S|small|id:I:?;"
    );
}
