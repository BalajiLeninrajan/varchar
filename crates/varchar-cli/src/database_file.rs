//! Database file loading, execution, and durable replacement.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use tempfile::NamedTempFile;
use varchar::{Database, Outcome};

use crate::CliResult;

pub(crate) fn init(path: &Path) -> CliResult<()> {
    let database = Database::new();
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| contextual_io("create database", path, error))?;

    file.write_all(database.as_str().as_bytes())
        .map_err(|error| contextual_io("write database", path, error))?;
    file.flush()
        .map_err(|error| contextual_io("flush database", path, error))?;
    file.sync_all()
        .map_err(|error| contextual_io("sync database", path, error))?;

    println!("initialized {}", path.display());
    Ok(())
}

pub(crate) fn load_database(path: &Path) -> CliResult<Database> {
    let blob =
        fs::read_to_string(path).map_err(|error| contextual_io("read database", path, error))?;
    Ok(Database::from_string(blob)?)
}

/// Execute against a disposable clone and only commit mutations after durable replacement.
pub(crate) fn execute_statement(
    database: &mut Database,
    path: &Path,
    sql: &str,
) -> CliResult<Outcome> {
    let mut candidate = database.clone();
    let outcome = candidate.execute(sql)?;

    if outcome.is_mutation() {
        persist_database(path, candidate.as_str())?;
        *database = candidate;
    }

    Ok(outcome)
}

fn persist_database(path: &Path, blob: &str) -> CliResult<()> {
    let parent = nonempty_parent(path);
    let mut temporary = NamedTempFile::new_in(parent)
        .map_err(|error| contextual_io("create temporary database", path, error))?;

    temporary
        .write_all(blob.as_bytes())
        .map_err(|error| contextual_io("write temporary database", path, error))?;
    temporary
        .flush()
        .map_err(|error| contextual_io("flush temporary database", path, error))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| contextual_io("sync temporary database", path, error))?;

    temporary
        .persist(path)
        .map_err(|error| contextual_io("replace database", path, error.error))?;
    Ok(())
}

fn nonempty_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn contextual_io(action: &str, path: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("could not {action} `{}`: {error}", path.display()),
    )
}
