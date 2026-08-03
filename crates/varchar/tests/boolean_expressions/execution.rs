use super::*;

#[test]
fn pushed_not_equal_and_like_keep_null_semantics() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE values_ (id INTEGER NOT NULL, note TEXT, touched BOOLEAN NOT NULL)",
    );
    for sql in [
        "INSERT INTO values_ VALUES (1, NULL, FALSE)",
        "INSERT INTO values_ VALUES (2, 'alpha', FALSE)",
        "INSERT INTO values_ VALUES (3, 'beta', FALSE)",
    ] {
        execute(&mut database, sql);
    }
    let mut database = Database::from_string(database.into_string()).expect("NULL fixture reloads");

    assert_eq!(
        rows(
            &mut database,
            "SELECT id FROM values_ WHERE note != 'alpha'",
        )
        .into_rows(),
        vec![vec![Value::Integer(3)]]
    );
    assert_eq!(
        rows(&mut database, "SELECT id FROM values_ WHERE note LIKE '%'").into_rows(),
        vec![vec![Value::Integer(2)], vec![Value::Integer(3)]]
    );
    assert_eq!(
        execute(
            &mut database,
            "UPDATE values_ SET touched = TRUE WHERE note != 'alpha'",
        ),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        execute(&mut database, "DELETE FROM values_ WHERE note LIKE 'a%'"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT id, note, touched FROM values_").into_rows(),
        vec![
            vec![Value::Integer(1), Value::Null, Value::Boolean(false),],
            vec![
                Value::Integer(3),
                Value::Text(String::from("beta")),
                Value::Boolean(true),
            ],
        ]
    );
}
