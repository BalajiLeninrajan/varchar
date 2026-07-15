use super::Database;
use crate::Error;
use crate::storage;

fn assert_catalog_current(database: &Database) {
    let reconstructed =
        storage::StorageState::load(database.as_str().to_owned()).expect("database remains valid");
    assert_eq!(database.storage, reconstructed);
}

#[test]
fn derived_catalog_tracks_every_commit() {
    let mut database = Database::new();
    assert_catalog_current(&database);

    for sql in [
        "CREATE TABLE t (id INTEGER PRIMARY KEY, note TEXT)",
        "INSERT INTO t VALUES (1, 'first')",
        "CREATE TABLE flags (enabled BOOLEAN NOT NULL)",
        "UPDATE t SET note = 'changed' WHERE id = 1",
        "DELETE FROM t WHERE id = 1",
    ] {
        database.execute(sql).expect("statement succeeds");
        assert_catalog_current(&database);
    }
}

#[test]
fn failed_constraint_validation_preserves_storage_state() {
    let mut database = Database::new();
    database
        .execute("CREATE TABLE t (id INTEGER PRIMARY KEY)")
        .expect("fixture schema succeeds");
    database
        .execute("INSERT INTO t VALUES (1)")
        .expect("fixture row succeeds");
    let before = database.storage.clone();

    assert!(matches!(
        database.execute("INSERT INTO t VALUES (1)"),
        Err(Error::Constraint(_))
    ));
    assert_eq!(database.storage, before);
}

#[test]
fn auto_increment_commits_keep_the_catalog_current_and_fail_atomically() {
    let mut database = Database::new();
    for sql in [
        "CREATE TABLE ids (id INTEGER PRIMARY KEY AUTOINCREMENT)",
        "INSERT INTO ids VALUES (NULL)",
        "UPDATE ids SET id = 10 WHERE id = 1",
        "INSERT INTO ids VALUES (NULL)",
    ] {
        database.execute(sql).expect("statement succeeds");
        assert_catalog_current(&database);
    }

    let before = database.storage.clone();
    assert!(matches!(
        database.execute("UPDATE ids SET id = 10 WHERE id = 11"),
        Err(Error::Constraint(_))
    ));
    assert_eq!(database.storage, before);
}

#[test]
fn debug_output_omits_the_derived_catalog() {
    let database = Database::new();
    assert!(!format!("{database:?}").contains("catalog"));
}
