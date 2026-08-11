use super::*;

#[test]
fn ordered_comparisons_cover_integer_text_boolean_and_null_left_values() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE ordered_values (\
             id INTEGER NOT NULL, \
             integer_value INTEGER, \
             text_value TEXT, \
             boolean_value BOOLEAN\
         )",
    );
    for sql in [
        "INSERT INTO ordered_values VALUES (1, -2, 'a', FALSE)",
        "INSERT INTO ordered_values VALUES (2, 0, 'é', TRUE)",
        "INSERT INTO ordered_values VALUES (3, 5, 'β', FALSE)",
        "INSERT INTO ordered_values VALUES (4, 9, '💾', TRUE)",
        "INSERT INTO ordered_values VALUES (5, NULL, NULL, NULL)",
        "INSERT INTO ordered_values VALUES (6, NULL, 'e\u{301}', NULL)",
    ] {
        execute(&mut database, sql);
    }

    for (sql, expected) in [
        (
            "SELECT id FROM ordered_values WHERE integer_value < 0",
            vec![vec![Value::Integer(1)]],
        ),
        (
            "SELECT id FROM ordered_values WHERE integer_value <= 0",
            vec![vec![Value::Integer(1)], vec![Value::Integer(2)]],
        ),
        (
            "SELECT id FROM ordered_values WHERE integer_value > 5",
            vec![vec![Value::Integer(4)]],
        ),
        (
            "SELECT id FROM ordered_values WHERE integer_value >= 5",
            vec![vec![Value::Integer(3)], vec![Value::Integer(4)]],
        ),
        (
            "SELECT id FROM ordered_values WHERE text_value < 'β'",
            vec![
                vec![Value::Integer(1)],
                vec![Value::Integer(2)],
                vec![Value::Integer(6)],
            ],
        ),
        (
            "SELECT id FROM ordered_values WHERE text_value < 'é'",
            vec![vec![Value::Integer(1)], vec![Value::Integer(6)]],
        ),
        (
            "SELECT id FROM ordered_values WHERE text_value >= 'β'",
            vec![vec![Value::Integer(3)], vec![Value::Integer(4)]],
        ),
        (
            "SELECT id FROM ordered_values WHERE boolean_value < TRUE",
            vec![vec![Value::Integer(1)], vec![Value::Integer(3)]],
        ),
        (
            "SELECT id FROM ordered_values WHERE boolean_value >= TRUE",
            vec![vec![Value::Integer(2)], vec![Value::Integer(4)]],
        ),
    ] {
        assert_eq!(rows(&mut database, sql).into_rows(), expected, "{sql}");
    }
}

#[test]
fn in_honors_null_duplicate_and_complete_membership_semantics() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE members (\
             id INTEGER NOT NULL, \
             number_value INTEGER, \
             text_value TEXT, \
             boolean_value BOOLEAN\
         )",
    );
    for sql in [
        "INSERT INTO members VALUES (1, 1, 'a', FALSE)",
        "INSERT INTO members VALUES (2, 2, 'b', TRUE)",
        "INSERT INTO members VALUES (3, NULL, NULL, NULL)",
    ] {
        execute(&mut database, sql);
    }
    let mut database =
        Database::from_string(database.into_string()).expect("membership fixture reloads");

    for (sql, expected) in [
        (
            "SELECT id FROM members WHERE number_value IN (NULL, 1)",
            vec![vec![Value::Integer(1)]],
        ),
        (
            "SELECT id FROM members WHERE number_value IN (1, NULL)",
            vec![vec![Value::Integer(1)]],
        ),
        (
            "SELECT id FROM members WHERE number_value IN (9, NULL)",
            Vec::new(),
        ),
        (
            "SELECT id FROM members WHERE number_value IN (2, 2)",
            vec![vec![Value::Integer(2)]],
        ),
        (
            "SELECT id FROM members WHERE text_value IN ('b', 'b')",
            vec![vec![Value::Integer(2)]],
        ),
        (
            "SELECT id FROM members WHERE boolean_value IN (FALSE, FALSE)",
            vec![vec![Value::Integer(1)]],
        ),
    ] {
        assert_eq!(rows(&mut database, sql).into_rows(), expected, "{sql}");
    }
}

#[test]
fn all_null_in_lists_are_valid_for_every_column_type() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE nullable_members (number_value INTEGER, text_value TEXT, boolean_value BOOLEAN)",
    );
    execute(
        &mut database,
        "INSERT INTO nullable_members VALUES (1, 'one', TRUE)",
    );
    execute(
        &mut database,
        "INSERT INTO nullable_members VALUES (NULL, NULL, NULL)",
    );

    for sql in [
        "SELECT number_value FROM nullable_members WHERE number_value IN (NULL, NULL)",
        "SELECT text_value FROM nullable_members WHERE text_value IN (NULL, NULL)",
        "SELECT boolean_value FROM nullable_members WHERE boolean_value IN (NULL, NULL)",
    ] {
        assert!(rows(&mut database, sql).into_rows().is_empty(), "{sql}");
    }
}
