#![cfg(not(target_family = "wasm"))]

use proptest::prelude::*;
use varchar::{ColumnOrigin, DataType, Database, Error, Outcome, ResultColumn, RowSet, Value};

#[derive(Clone, Copy, Debug)]
enum SelectedColumn {
    Text,
    Number,
    Flag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ModelRow {
    text: Option<String>,
    number: i64,
    flag: bool,
}

#[derive(Clone, Debug)]
enum LikeAtom {
    Literal(char),
    One,
    Many,
}

#[derive(Clone, Debug)]
enum Predicate {
    TextEqual(String),
    TextNotEqual(String),
    TextLike(Vec<LikeAtom>),
    TextIsNull,
    TextIsNotNull,
    NumberEqual(i64),
    NumberNotEqual(i64),
    FlagEqual(bool),
    FlagNotEqual(bool),
}

#[derive(Clone, Debug)]
enum CrudOperation {
    Insert { id: i8, text: Option<String> },
    Update { id: i8, text: Option<String> },
    Delete { id: i8 },
}

fn interesting_character() -> impl Strategy<Value = char> {
    prop_oneof![
        8 => any::<char>(),
        2 => prop::sample::select(vec![
            '%', '|', ';', '~', '\\', '\'', '\0', '\n', '\r', '\t', '\u{2028}', '\u{2029}',
            'é', '\u{301}', '💾', '.', '*', '[', ']', '(', ')', '^', '$',
        ]),
    ]
}

fn interesting_text(max_length: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(interesting_character(), 0..=max_length)
        .prop_map(|characters| characters.into_iter().collect())
}

fn nullable_text(max_length: usize) -> impl Strategy<Value = Option<String>> {
    prop_oneof![1 => Just(None), 3 => interesting_text(max_length).prop_map(Some)]
}

fn model_row() -> impl Strategy<Value = ModelRow> {
    (
        nullable_text(10),
        prop_oneof![
            8 => -3_i64..=3,
            1 => Just(i64::MIN),
            1 => Just(i64::MAX),
            1 => any::<i64>(),
        ],
        any::<bool>(),
    )
        .prop_map(|(text, number, flag)| ModelRow { text, number, flag })
}

fn selected_column() -> impl Strategy<Value = SelectedColumn> {
    prop::sample::select(vec![
        SelectedColumn::Text,
        SelectedColumn::Number,
        SelectedColumn::Flag,
    ])
}

fn like_atom() -> impl Strategy<Value = LikeAtom> {
    prop_oneof![
        5 => interesting_character().prop_map(LikeAtom::Literal),
        2 => Just(LikeAtom::One),
        2 => Just(LikeAtom::Many),
    ]
}

fn predicate() -> impl Strategy<Value = Predicate> {
    prop_oneof![
        3 => interesting_text(7).prop_map(Predicate::TextEqual),
        3 => interesting_text(7).prop_map(Predicate::TextNotEqual),
        4 => prop::collection::vec(like_atom(), 0..=7).prop_map(Predicate::TextLike),
        1 => Just(Predicate::TextIsNull),
        1 => Just(Predicate::TextIsNotNull),
        3 => (-3_i64..=3).prop_map(Predicate::NumberEqual),
        3 => (-3_i64..=3).prop_map(Predicate::NumberNotEqual),
        2 => any::<bool>().prop_map(Predicate::FlagEqual),
        2 => any::<bool>().prop_map(Predicate::FlagNotEqual),
    ]
}

fn crud_operation() -> impl Strategy<Value = CrudOperation> {
    prop_oneof![
        4 => (any::<i8>(), nullable_text(8)).prop_map(|(id, text)| CrudOperation::Insert {
            id,
            text,
        }),
        3 => (any::<i8>(), nullable_text(8)).prop_map(|(id, text)| CrudOperation::Update {
            id,
            text,
        }),
        2 => any::<i8>().prop_map(|id| CrudOperation::Delete { id }),
    ]
}

fn sql_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn sql_nullable_text(value: &Option<String>) -> String {
    value.as_deref().map_or_else(|| "NULL".to_owned(), sql_text)
}

fn sql_boolean(value: bool) -> &'static str {
    if value { "TRUE" } else { "FALSE" }
}

fn selected_column_name(column: SelectedColumn) -> &'static str {
    match column {
        SelectedColumn::Text => "txt",
        SelectedColumn::Number => "n",
        SelectedColumn::Flag => "flag",
    }
}

fn selected_column_metadata(column: SelectedColumn) -> ResultColumn {
    match column {
        SelectedColumn::Text => result_column("txt", DataType::Text, true),
        SelectedColumn::Number => result_column("n", DataType::Integer, false),
        SelectedColumn::Flag => result_column("flag", DataType::Boolean, false),
    }
}

fn result_column(name: &str, data_type: DataType, nullable: bool) -> ResultColumn {
    ResultColumn::new(
        name.to_owned(),
        ColumnOrigin::new("t".to_owned(), name.to_owned()),
        data_type,
        nullable,
    )
}

fn selected_value(row: &ModelRow, column: SelectedColumn) -> Value {
    match column {
        SelectedColumn::Text => row
            .text
            .as_ref()
            .map_or(Value::Null, |value| Value::Text(value.clone())),
        SelectedColumn::Number => Value::Integer(row.number),
        SelectedColumn::Flag => Value::Boolean(row.flag),
    }
}

fn render_like_pattern(atoms: &[LikeAtom]) -> String {
    let mut pattern = String::new();
    for atom in atoms {
        match atom {
            LikeAtom::One => pattern.push('_'),
            LikeAtom::Many => pattern.push('%'),
            LikeAtom::Literal(character) => {
                if matches!(character, '%' | '_' | '\\') {
                    pattern.push('\\');
                }
                pattern.push(*character);
            }
        }
    }
    sql_text(&pattern)
}

fn render_predicate(predicate: &Predicate) -> String {
    match predicate {
        Predicate::TextEqual(value) => format!("txt = {}", sql_text(value)),
        Predicate::TextNotEqual(value) => format!("txt != {}", sql_text(value)),
        Predicate::TextLike(atoms) => format!("txt LIKE {}", render_like_pattern(atoms)),
        Predicate::TextIsNull => "txt IS NULL".to_owned(),
        Predicate::TextIsNotNull => "txt IS NOT NULL".to_owned(),
        Predicate::NumberEqual(value) => format!("n = {value}"),
        Predicate::NumberNotEqual(value) => format!("n != {value}"),
        Predicate::FlagEqual(value) => format!("flag = {}", sql_boolean(*value)),
        Predicate::FlagNotEqual(value) => format!("flag != {}", sql_boolean(*value)),
    }
}

fn predicate_matches(row: &ModelRow, predicate: &Predicate) -> bool {
    match predicate {
        Predicate::TextEqual(expected) => row.text.as_ref() == Some(expected),
        Predicate::TextNotEqual(expected) => {
            row.text.as_ref().is_some_and(|actual| actual != expected)
        }
        Predicate::TextLike(pattern) => row
            .text
            .as_deref()
            .is_some_and(|actual| like_matches(actual, pattern)),
        Predicate::TextIsNull => row.text.is_none(),
        Predicate::TextIsNotNull => row.text.is_some(),
        Predicate::NumberEqual(expected) => row.number == *expected,
        Predicate::NumberNotEqual(expected) => row.number != *expected,
        Predicate::FlagEqual(expected) => row.flag == *expected,
        Predicate::FlagNotEqual(expected) => row.flag != *expected,
    }
}

fn like_matches(value: &str, pattern: &[LikeAtom]) -> bool {
    let characters: Vec<char> = value.chars().collect();
    let mut reachable = vec![false; characters.len() + 1];
    reachable[0] = true;

    for atom in pattern {
        let mut next = vec![false; characters.len() + 1];
        match atom {
            LikeAtom::Literal(expected) => {
                for index in 0..characters.len() {
                    if reachable[index] && characters[index] == *expected {
                        next[index + 1] = true;
                    }
                }
            }
            LikeAtom::One => {
                for index in 0..characters.len() {
                    if reachable[index] {
                        next[index + 1] = true;
                    }
                }
            }
            LikeAtom::Many => {
                let mut any_prefix_reachable = false;
                for index in 0..=characters.len() {
                    any_prefix_reachable |= reachable[index];
                    next[index] = any_prefix_reachable;
                }
            }
        }
        reachable = next;
    }

    reachable[characters.len()]
}

fn rows_from_outcome(outcome: Outcome) -> RowSet {
    match outcome {
        Outcome::Rows(rows) => rows,
        other => panic!("expected rows, got {other:?}"),
    }
}

fn affected_rows(outcome: Outcome) -> usize {
    match outcome {
        Outcome::Affected { rows } => rows,
        other => panic!("expected an affected-row count, got {other:?}"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        max_shrink_iters: 2_048,
        ..ProptestConfig::default()
    })]

    #[test]
    fn generated_regex_agrees_with_a_direct_predicate_evaluator(
        rows in prop::collection::vec(model_row(), 0..=12),
        projection in prop::collection::vec(selected_column(), 1..=6),
        predicates in prop::collection::vec(predicate(), 0..=5),
    ) {
        let mut database = Database::new();
        database
            .execute("CREATE TABLE t (txt TEXT, n INTEGER NOT NULL, flag BOOLEAN NOT NULL)")
            .unwrap();
        for row in &rows {
            let sql = format!(
                "INSERT INTO t VALUES ({}, {}, {})",
                sql_nullable_text(&row.text),
                row.number,
                sql_boolean(row.flag),
            );
            prop_assert_eq!(database.execute(&sql).unwrap(), Outcome::Affected { rows: 1 });
        }

        let projection_sql = projection
            .iter()
            .copied()
            .map(selected_column_name)
            .collect::<Vec<_>>()
            .join(", ");
        let mut sql = format!("SELECT {projection_sql} FROM t");
        if !predicates.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(
                &predicates
                    .iter()
                    .map(render_predicate)
                    .collect::<Vec<_>>()
                    .join(" AND "),
            );
        }

        let expected_columns = projection
            .iter()
            .copied()
            .map(selected_column_metadata)
            .collect::<Vec<_>>();
        let expected_rows = rows
            .iter()
            .filter(|row| predicates.iter().all(|predicate| predicate_matches(row, predicate)))
            .map(|row| {
                projection
                    .iter()
                    .copied()
                    .map(|column| selected_value(row, column))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        let before = database.as_str().to_owned();
        let plan = database.explain_select(&sql).unwrap();
        prop_assert_eq!(plan.sources(), &["t"]);
        prop_assert!(!plan.pattern().is_empty());
        prop_assert_eq!(plan.columns(), expected_columns.as_slice());
        prop_assert_eq!(database.as_str(), &before);

        prop_assert_eq!(
            database.execute(&format!("EXPLAIN REGEX {sql}")).unwrap(),
            Outcome::Explain(plan),
        );
        prop_assert_eq!(database.as_str(), &before);

        prop_assert_eq!(
            database.execute(&sql).unwrap(),
            Outcome::Rows(RowSet::new(expected_columns, expected_rows)),
        );
        prop_assert_eq!(database.as_str(), &before);
    }

    #[test]
    fn arbitrary_unicode_round_trips_through_canonical_storage(value in interesting_text(48)) {
        let mut database = Database::new();
        database
            .execute("CREATE TABLE strings (value TEXT NOT NULL)")
            .unwrap();
        database
            .execute(&format!("INSERT INTO strings VALUES ({})", sql_text(&value)))
            .unwrap();

        let blob = database.into_string();
        prop_assert!(!blob.chars().any(char::is_control));
        prop_assert!(!blob.contains('\u{2028}'), "blob contains a raw line separator");
        prop_assert!(!blob.contains('\u{2029}'), "blob contains a raw paragraph separator");

        let mut reloaded = Database::from_string(blob.clone()).unwrap();
        prop_assert_eq!(reloaded.as_str(), &blob);
        prop_assert_eq!(
            rows_from_outcome(reloaded.execute("SELECT value FROM strings").unwrap()).into_rows(),
            vec![vec![Value::Text(value)]],
        );

        let mut truncated = blob;
        prop_assert_eq!(truncated.pop(), Some(';'));
        prop_assert!(matches!(
            Database::from_string(truncated),
            Err(Error::CorruptStorage { .. })
        ), "truncated storage was accepted");
    }

    #[test]
    fn arbitrary_sql_never_panics_and_errors_never_mutate(
        sql in prop::collection::vec(any::<char>(), 0..=128)
            .prop_map(|characters| characters.into_iter().collect::<String>()),
    ) {
        let mut database = Database::new();
        database.execute("CREATE TABLE seeded (id INTEGER)").unwrap();
        database.execute("INSERT INTO seeded VALUES (1)").unwrap();

        let before_execute = database.as_str().to_owned();
        if database.execute(&sql).is_err() {
            prop_assert_eq!(database.as_str(), &before_execute);
        }

        let before_compile = database.as_str().to_owned();
        let _ = database.explain_select(&sql);
        prop_assert_eq!(database.as_str(), &before_compile);
    }

    #[test]
    fn randomized_crud_traces_match_a_vec_model(
        operations in prop::collection::vec(crud_operation(), 0..=24),
    ) {
        let mut database = Database::new();
        database
            .execute("CREATE TABLE items (id INTEGER NOT NULL, txt TEXT)")
            .unwrap();
        let mut model: Vec<(i8, Option<String>)> = Vec::new();

        for operation in operations {
            match operation {
                CrudOperation::Insert { id, text } => {
                    let sql = format!(
                        "INSERT INTO items VALUES ({id}, {})",
                        sql_nullable_text(&text),
                    );
                    prop_assert_eq!(affected_rows(database.execute(&sql).unwrap()), 1);
                    model.push((id, text));
                }
                CrudOperation::Update { id, text } => {
                    let sql = format!(
                        "UPDATE items SET txt = {} WHERE id = {id}",
                        sql_nullable_text(&text),
                    );
                    let expected = model.iter().filter(|(row_id, _)| *row_id == id).count();
                    prop_assert_eq!(affected_rows(database.execute(&sql).unwrap()), expected);
                    for (_, row_text) in model.iter_mut().filter(|(row_id, _)| *row_id == id) {
                        *row_text = text.clone();
                    }
                }
                CrudOperation::Delete { id } => {
                    let sql = format!("DELETE FROM items WHERE id = {id}");
                    let expected = model.iter().filter(|(row_id, _)| *row_id == id).count();
                    prop_assert_eq!(affected_rows(database.execute(&sql).unwrap()), expected);
                    model.retain(|(row_id, _)| *row_id != id);
                }
            }

            let before_invalid = database.as_str().to_owned();
            prop_assert!(database
                .execute("UPDATE items SET id = 'not an integer' WHERE id = 0")
                .is_err());
            prop_assert_eq!(database.as_str(), &before_invalid);
        }

        let expected = model
            .into_iter()
            .map(|(id, text)| {
                vec![
                    Value::Integer(i64::from(id)),
                    text.map_or(Value::Null, Value::Text),
                ]
            })
            .collect::<Vec<_>>();
        prop_assert_eq!(
            rows_from_outcome(database.execute("SELECT * FROM items").unwrap()).into_rows(),
            expected,
        );
    }
}
