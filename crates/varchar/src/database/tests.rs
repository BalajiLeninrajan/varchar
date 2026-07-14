use super::Database;
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
        "CREATE TABLE t (id INTEGER NOT NULL, note TEXT)",
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
fn debug_output_omits_the_derived_catalog() {
    let database = Database::new();
    assert!(!format!("{database:?}").contains("catalog"));
}
