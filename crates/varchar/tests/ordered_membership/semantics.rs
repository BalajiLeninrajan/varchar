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
