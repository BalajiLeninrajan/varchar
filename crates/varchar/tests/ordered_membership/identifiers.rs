use super::*;

fn assert_reserved(database: &mut Database, sql: &str, keyword: &str, marker: &str) {
    let before = database.as_str().to_owned();
    let span_start = sql.find(marker).expect("fixture contains error marker");
    assert!(
        matches!(
            database.execute(sql),
            Err(Error::Parse {
                ref message,
                span_start: actual_start,
                span_end: actual_end,
            }) if message == &format!("reserved keyword `{keyword}` cannot be used as an identifier")
                && (actual_start, actual_end) == (span_start, span_start + marker.len())
        ),
        "expected {keyword} to be rejected as an identifier in {sql:?}"
    );
    assert_eq!(database.as_str(), before);
}

#[test]
fn in_is_reserved_and_is_rejected_as_a_public_identifier() {
    let mut database = Database::new();

    for (sql, marker) in [
        ("CREATE TABLE in (id INTEGER NOT NULL)", "in"),
        ("CREATE TABLE memberships (in INTEGER NOT NULL)", "in"),
    ] {
        assert_reserved(&mut database, sql, "IN", marker);
    }

    execute(
        &mut database,
        "CREATE TABLE memberships (id INTEGER NOT NULL)",
    );
    execute(&mut database, "INSERT INTO memberships (id) VALUES (1)");
    execute(&mut database, "INSERT INTO memberships (id) VALUES (2)");

    for (sql, marker) in [
        ("INSERT INTO in (id) VALUES (1)", "in"),
        ("SELECT in FROM memberships", "in"),
        ("SELECT id FROM in", "in"),
        ("SELECT id FROM memberships WHERE in = 1", "in"),
        ("UPDATE in SET id = 3", "in"),
        ("UPDATE memberships SET in = 3", "in"),
        ("DELETE FROM in", "in"),
        ("DELETE FROM memberships WHERE in < 3", "in"),
    ] {
        assert_reserved(&mut database, sql, "IN", marker);
    }

    // The keyword still drives the membership predicate it was reserved for.
    assert_eq!(
        rows(&mut database, "SELECT id FROM memberships WHERE id IN (2)").into_rows(),
        vec![vec![Value::Integer(2)]]
    );
    assert_eq!(
        execute(
            &mut database,
            "UPDATE memberships SET id = 3 WHERE id IN (1)"
        ),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        execute(&mut database, "DELETE FROM memberships WHERE id < 3"),
        Outcome::Affected { rows: 1 }
    );
    assert_eq!(
        rows(&mut database, "SELECT id FROM memberships").into_rows(),
        vec![vec![Value::Integer(3)]]
    );
}
