use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn command() -> Command {
    Command::cargo_bin("varchar").expect("varchar binary should build")
}

fn initialized_database() -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let path = directory.path().join("test.varchar");
    command()
        .arg("init")
        .arg(&path)
        .assert()
        .success()
        .stdout(predicate::str::contains("initialized"));
    (directory, path)
}

fn exec(path: &Path, sql: &str) -> assert_cmd::assert::Assert {
    command().arg("exec").arg(path).arg(sql).assert()
}

#[test]
fn init_refuses_to_overwrite_an_existing_database() {
    let (_directory, path) = initialized_database();
    let original = fs::read(&path).expect("database should be readable");

    command()
        .arg("init")
        .arg(&path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("could not create database"));

    assert_eq!(
        fs::read(path).expect("database should remain readable"),
        original
    );
}

#[test]
fn exec_supports_arguments_stdin_queries_and_validated_dumping() {
    let (_directory, path) = initialized_database();

    exec(
        &path,
        "CREATE TABLE jokes (id INTEGER NOT NULL, setup TEXT, funny BOOLEAN)",
    )
    .success()
    .stdout("created table jokes\n");

    command()
        .arg("exec")
        .arg(&path)
        .write_stdin("INSERT INTO jokes VALUES (1, 'regex walks into a bar', true)")
        .assert()
        .success()
        .stdout("affected 1 row\n");

    let blob_before_read = fs::read(&path).expect("database should be readable");
    #[cfg(unix)]
    let inode_before_read = fs::metadata(&path)
        .expect("database metadata should be readable")
        .ino();
    exec(&path, "SELECT setup, funny FROM jokes WHERE id = 1")
        .success()
        .stdout(
            predicate::str::contains("regex walks into a bar")
                .and(predicate::str::contains("true"))
                .and(predicate::str::contains("1 row")),
        );
    assert_eq!(
        fs::read(&path).expect("database should remain readable"),
        blob_before_read,
        "a read must not rewrite the database"
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&path)
            .expect("database metadata should remain readable")
            .ino(),
        inode_before_read,
        "a read must not replace the database file"
    );

    command()
        .arg("dump")
        .arg(&path)
        .assert()
        .success()
        .stdout(format!(
            "{}\n",
            String::from_utf8(blob_before_read).expect("database is UTF-8")
        ));
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&path)
            .expect("database metadata should remain readable")
            .ino(),
        inode_before_read,
        "dump must not replace the database file"
    );
}

#[test]
fn failed_execution_preserves_the_previous_database() {
    let (_directory, path) = initialized_database();
    exec(&path, "CREATE TABLE items (id INTEGER NOT NULL)")
        .success()
        .stdout("created table items\n");
    let original = fs::read(&path).expect("database should be readable");

    exec(&path, "INSERT INTO items VALUES (NULL)")
        .failure()
        .stderr(predicate::str::contains("error:"));

    assert_eq!(
        fs::read(path).expect("database should remain readable"),
        original
    );
}

#[test]
fn auto_increment_state_persists_across_cli_processes() {
    let (_directory, path) = initialized_database();
    exec(
        &path,
        "CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT NOT NULL)",
    )
    .success();
    exec(&path, "INSERT INTO messages (body) VALUES ('first')").success();
    exec(&path, "INSERT INTO messages (body) VALUES ('second')").success();

    exec(&path, "SELECT id, body FROM messages")
        .success()
        .stdout(
            predicate::str::contains("first")
                .and(predicate::str::contains("second"))
                .and(predicate::str::contains("2 rows")),
        );
    command()
        .arg("dump")
        .arg(path)
        .assert()
        .success()
        .stdout(predicate::str::contains("~A|messages|id|I2;"));
}

#[test]
fn inner_joins_execute_across_persisted_cli_commands() {
    let (_directory, path) = initialized_database();
    exec(
        &path,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
    )
    .success();
    exec(
        &path,
        "CREATE TABLE children (parent_id INTEGER REFERENCES parents(id), name TEXT NOT NULL)",
    )
    .success();
    exec(&path, "INSERT INTO parents VALUES (1, 'parent')").success();
    exec(&path, "INSERT INTO children VALUES (1, 'child')").success();

    exec(
        &path,
        "SELECT parents.name, children.name FROM parents \
         JOIN children ON parents.id = children.parent_id",
    )
    .success()
    .stdout(
        predicate::str::contains("| parents.name | children.name |")
            .and(predicate::str::contains("parent"))
            .and(predicate::str::contains("child"))
            .and(predicate::str::contains("1 row")),
    );
}

#[test]
fn explain_regex_reports_whether_the_pattern_is_exact() {
    let (_directory, path) = initialized_database();
    exec(
        &path,
        "CREATE TABLE parents (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
    )
    .success();
    exec(
        &path,
        "CREATE TABLE children (parent_id INTEGER REFERENCES parents(id), name TEXT NOT NULL)",
    )
    .success();

    exec(&path, "EXPLAIN REGEX SELECT id FROM parents WHERE id = 1")
        .success()
        .stdout(
            predicate::str::starts_with("regex: ").and(predicate::str::ends_with("rows: exact\n")),
        );

    // A residual factor and a join each leave the pattern a prefilter.
    exec(
        &path,
        "EXPLAIN REGEX SELECT id FROM parents WHERE id = 1 OR id = 2",
    )
    .success()
    .stdout(predicate::str::ends_with(
        "rows: prefilter (Rust-side filtering applies)\n",
    ));
    exec(
        &path,
        "EXPLAIN REGEX SELECT parents.name, children.name FROM parents \
         JOIN children ON parents.id = children.parent_id",
    )
    .success()
    .stdout(predicate::str::ends_with(
        "rows: prefilter (Rust-side filtering applies)\n",
    ));
}

#[cfg(unix)]
#[test]
fn failed_persistence_preserves_the_previous_database() {
    let (directory, path) = initialized_database();
    let original = fs::read(&path).expect("database should be readable");
    let parent = directory.path();
    let original_permissions = fs::metadata(parent)
        .expect("directory metadata should be readable")
        .permissions();
    let mut unwritable_permissions = original_permissions.clone();
    unwritable_permissions.set_mode(original_permissions.mode() & !0o222);
    fs::set_permissions(parent, unwritable_permissions)
        .expect("directory should become unwritable");

    let output = command()
        .arg("exec")
        .arg(&path)
        .arg("CREATE TABLE items (id INTEGER NOT NULL)")
        .output();

    fs::set_permissions(parent, original_permissions)
        .expect("directory permissions should be restored");
    let output = output.expect("varchar should run");

    assert!(!output.status.success(), "persistence should fail");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("could not create temporary database"),
        "stderr should describe the persistence failure: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(&path).expect("database should remain readable"),
        original
    );
}

#[test]
fn shell_accepts_multiline_statements_and_meta_commands() {
    let (_directory, path) = initialized_database();

    command()
        .arg("shell")
        .arg(&path)
        .write_stdin("CREATE TABLE notes (\nbody TEXT\n);\n.dump\n.quit\n")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("created table notes").and(predicate::str::contains("V2;")),
        );

    exec(&path, "SELECT * FROM notes")
        .success()
        .stdout(predicate::str::contains("0 rows"));
}

#[test]
fn dump_rejects_corrupt_storage() {
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let path = directory.path().join("corrupt.varchar");
    fs::write(&path, "not a varchar database").expect("fixture should be written");

    command()
        .arg("dump")
        .arg(path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("corrupt database"));
}
