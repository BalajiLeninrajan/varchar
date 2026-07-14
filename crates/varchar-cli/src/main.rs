use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tempfile::NamedTempFile;
use varchar::{Database, Outcome, RowSet, Value};

type CliResult<T> = Result<T, Box<dyn StdError>>;

#[derive(Debug, Parser)]
#[command(
    name = "varchar",
    version,
    about = "A database stored in one string and queried with regex"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a new, empty database file.
    Init { file: PathBuf },

    /// Open an interactive SQL shell.
    Shell { file: PathBuf },

    /// Execute one SQL statement.
    Exec {
        file: PathBuf,

        /// SQL to execute. Reads from standard input when omitted.
        #[arg(trailing_var_arg = true)]
        sql: Vec<String>,
    },

    /// Print the raw one-string database after validating it.
    Dump { file: PathBuf },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> CliResult<()> {
    match cli.command {
        Command::Init { file } => init(&file),
        Command::Shell { file } => shell(&file),
        Command::Exec { file, sql } => exec(&file, sql),
        Command::Dump { file } => dump(&file),
    }
}

fn init(path: &Path) -> CliResult<()> {
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

fn exec(path: &Path, sql: Vec<String>) -> CliResult<()> {
    let sql = if sql.is_empty() {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        input
    } else {
        sql.join(" ")
    };

    let mut database = load_database(path)?;
    let outcome = execute_statement(&mut database, path, &sql)?;
    print_outcome(&outcome)?;
    Ok(())
}

fn dump(path: &Path) -> CliResult<()> {
    let database = load_database(path)?;
    println!("{}", database.as_str());
    Ok(())
}

fn shell(path: &Path) -> CliResult<()> {
    let mut database = load_database(path)?;
    let stdin = io::stdin();
    let interactive = stdin.is_terminal();
    let mut input = stdin.lock();
    let mut statement = String::new();

    loop {
        if interactive {
            if statement.is_empty() {
                print!("varchar> ");
            } else {
                print!("       > ");
            }
            io::stdout().flush()?;
        }

        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            if statement.trim().is_empty() {
                if interactive {
                    println!();
                }
                return Ok(());
            }
            return Err("incomplete SQL statement at end of input (expected `;`)".into());
        }

        if statement.is_empty() {
            match line.trim() {
                ".quit" => return Ok(()),
                ".dump" => {
                    println!("{}", database.as_str());
                    continue;
                }
                _ => {}
            }

            if line.trim().is_empty() {
                continue;
            }
        }

        statement.push_str(&line);
        if !is_semicolon_complete(&statement) {
            continue;
        }

        match execute_statement(&mut database, path, &statement) {
            Ok(outcome) => {
                print_outcome(&outcome)?;
            }
            Err(error) => eprintln!("error: {error}"),
        }
        statement.clear();
    }
}

fn load_database(path: &Path) -> CliResult<Database> {
    let blob =
        fs::read_to_string(path).map_err(|error| contextual_io("read database", path, error))?;
    Ok(Database::from_string(blob)?)
}

/// Execute against a disposable clone and only commit mutations after durable replacement.
fn execute_statement(database: &mut Database, path: &Path, sql: &str) -> CliResult<Outcome> {
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

fn is_semicolon_complete(input: &str) -> bool {
    let mut chars = input.char_indices().peekable();
    let mut in_string = false;
    let mut terminal_semicolon = false;

    while let Some((_, character)) = chars.next() {
        if character == '\'' {
            if in_string && chars.peek().is_some_and(|(_, next)| *next == '\'') {
                chars.next();
            } else {
                in_string = !in_string;
            }
            terminal_semicolon = false;
        } else if !in_string && character == ';' {
            terminal_semicolon = true;
        } else if !character.is_whitespace() {
            terminal_semicolon = false;
        }
    }

    !in_string && terminal_semicolon
}

fn print_outcome(outcome: &Outcome) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();

    match outcome {
        Outcome::Rows(rows) => print_rows(&mut output, rows),
        Outcome::Affected { rows } => writeln!(
            output,
            "affected {rows} row{}",
            if *rows == 1 { "" } else { "s" }
        ),
        Outcome::Created { table } => writeln!(output, "created table {table}"),
        Outcome::Explain(plan) => writeln!(output, "regex: {}", plan.pattern()),
    }
}

fn print_rows(output: &mut impl Write, row_set: &RowSet) -> io::Result<()> {
    let rendered_rows: Vec<Vec<String>> = row_set
        .rows()
        .iter()
        .map(|row| row.iter().map(render_value).collect())
        .collect();
    let headers = result_headers(row_set);
    let mut widths: Vec<usize> = headers.iter().map(|header| display_width(header)).collect();

    for row in &rendered_rows {
        for (index, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(display_width(cell));
            }
        }
    }

    write_border(output, &widths)?;
    write_cells(
        output,
        &headers.iter().map(String::as_str).collect::<Vec<_>>(),
        &widths,
    )?;
    write_border(output, &widths)?;
    for row in &rendered_rows {
        write_cells(
            output,
            &row.iter().map(String::as_str).collect::<Vec<_>>(),
            &widths,
        )?;
    }
    write_border(output, &widths)?;
    writeln!(
        output,
        "{} row{}",
        row_set.rows().len(),
        if row_set.rows().len() == 1 { "" } else { "s" }
    )
}

fn result_headers(row_set: &RowSet) -> Vec<String> {
    disambiguate_headers(row_set.columns().iter().map(|column| {
        (
            column.label(),
            column.origin().table(),
            column.origin().column(),
        )
    }))
}

fn disambiguate_headers<'a>(
    columns: impl IntoIterator<Item = (&'a str, &'a str, &'a str)>,
) -> Vec<String> {
    let columns = columns.into_iter().collect::<Vec<_>>();
    let mut labels = BTreeMap::new();

    for &(label, table, column) in &columns {
        labels
            .entry(label)
            .and_modify(|(first_origin, ambiguous)| {
                *ambiguous |= *first_origin != (table, column);
            })
            .or_insert(((table, column), false));
    }

    columns
        .into_iter()
        .map(|(label, table, column)| {
            let ambiguous = labels.get(label).is_some_and(|(_, ambiguous)| *ambiguous);
            if ambiguous {
                format!("{table}.{column}")
            } else {
                label.to_owned()
            }
        })
        .collect()
}

fn render_value(value: &Value) -> String {
    match value {
        Value::Text(text) => text
            .replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t"),
        Value::Integer(integer) => integer.to_string(),
        Value::Boolean(boolean) => boolean.to_string(),
        Value::Null => "NULL".to_owned(),
    }
}

fn display_width(value: &str) -> usize {
    value.chars().count()
}

fn write_border(output: &mut impl Write, widths: &[usize]) -> io::Result<()> {
    write!(output, "+")?;
    for width in widths {
        write!(output, "-{:-<width$}-+", "", width = width)?;
    }
    writeln!(output)
}

fn write_cells(output: &mut impl Write, cells: &[&str], widths: &[usize]) -> io::Result<()> {
    write!(output, "|")?;
    for (cell, width) in cells.iter().zip(widths) {
        let padding = width.saturating_sub(display_width(cell));
        write!(output, " {cell}{} |", " ".repeat(padding))?;
    }
    writeln!(output)
}

#[cfg(test)]
mod tests;
