use super::*;

#[test]
fn ordered_comparisons_reject_null_and_cross_type_literals_before_execution() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE typed_values (id INTEGER NOT NULL, note TEXT, enabled BOOLEAN NOT NULL)",
    );
    execute(
        &mut database,
        "INSERT INTO typed_values VALUES (1, 'one', TRUE)",
    );
    let before = database.as_str().to_owned();

    for sql in [
        "SELECT id FROM typed_values WHERE id < NULL",
        "SELECT id FROM typed_values WHERE id <= NULL",
        "SELECT id FROM typed_values WHERE id > NULL",
        "SELECT id FROM typed_values WHERE id >= NULL",
    ] {
        assert!(matches!(
            database.execute(sql),
            Err(Error::Type(ref message))
                if message
                    == "NULL cannot be compared with `<`, `<=`, `>`, or `>=`; use IS NULL or IS NOT NULL"
        ));
        assert_eq!(database.as_str(), before, "{sql}");
    }

    assert!(matches!(
        database.execute("SELECT id FROM typed_values WHERE enabled >= 1"),
        Err(Error::Type(ref message))
            if message == "column \"enabled\" expects BOOLEAN, got INTEGER"
    ));
    assert_eq!(database.as_str(), before);

    assert!(matches!(
        database.execute("SELECT id FROM typed_values WHERE note > FALSE"),
        Err(Error::Type(ref message))
            if message == "column \"note\" expects TEXT, got BOOLEAN"
    ));
    assert_eq!(database.as_str(), before);
}

#[test]
fn existing_name_equality_and_like_error_contracts_remain_compatible() {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE compatibility_values (id INTEGER NOT NULL, note TEXT)",
    );
    let before = database.as_str().to_owned();

    for sql in [
        "SELECT id FROM compatibility_values WHERE id = NULL",
        "SELECT id FROM compatibility_values WHERE id != NULL",
    ] {
        assert!(matches!(
            database.execute(sql),
            Err(Error::Type(ref message))
                if message
                    == "NULL cannot be compared with `=` or `!=`; use IS NULL or IS NOT NULL"
        ));
        assert_eq!(database.as_str(), before, "{sql}");
    }

    assert!(matches!(
        database.execute("SELECT id FROM compatibility_values WHERE id LIKE '1'"),
        Err(Error::Type(ref message))
            if message == "LIKE requires a TEXT column; \"id\" is INTEGER"
    ));
    assert_eq!(database.as_str(), before);

    assert!(matches!(
        database.execute("SELECT id FROM compatibility_values WHERE id = 'wrong'"),
        Err(Error::Type(ref message))
            if message == "column \"id\" expects INTEGER, got TEXT"
    ));
    assert_eq!(database.as_str(), before);

    assert!(matches!(
        database.execute("SELECT id FROM compatibility_values WHERE missing < NULL"),
        Err(Error::Schema(ref message))
            if message == "unknown column \"missing\" in table \"compatibility_values\""
    ));
    assert_eq!(database.as_str(), before);
}
