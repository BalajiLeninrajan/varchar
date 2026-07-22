mod database_file;
mod output;
mod shell;

use std::error::Error as StdError;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use database_file::{execute_statement, init, load_database};
use output::print_outcome;
use shell::shell;

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
