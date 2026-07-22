//! Interactive shell input, prompts, and meta commands.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

use crate::CliResult;
use crate::database_file::{execute_statement, load_database};
use crate::output::print_outcome;

pub(crate) fn shell(path: &Path) -> CliResult<()> {
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
