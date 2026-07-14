use super::Database;
use crate::{Error, storage};

fn assert_catalog_current(database: &Database) {
    let reconstructed =
        storage::validate_and_catalog(database.as_str()).expect("database remains valid");
    assert_eq!(database.catalog, reconstructed);
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
fn failed_candidate_validation_preserves_blob_and_catalog() {
    let mut database = Database::new();
    database
        .execute("CREATE TABLE t (id INTEGER)")
        .expect("fixture schema succeeds");
    let before_blob = database.blob.clone();
    let before_catalog = database.catalog.clone();

    assert!(matches!(
        database.commit_candidate(String::from("V1;garbage")),
        Err(Error::CorruptStorage { .. })
    ));
    assert_eq!(database.blob, before_blob);
    assert_eq!(database.catalog, before_catalog);
}

#[test]
fn debug_output_omits_the_derived_catalog() {
    let database = Database::new();
    assert!(!format!("{database:?}").contains("catalog"));
}
