mod check;

use std::fmt;

use crate::expression::format_value;
use crate::sql::is_reserved_identifier;
use crate::storage::{
    AutoIncrement, ForeignKey, ForeignKeyDeleteAction, ForeignKeyUpdateAction, TableSchema,
};
use crate::{Error, Result, Value};

pub(super) fn measure(
    table: &TableSchema,
    auto_increment: Option<AutoIncrement>,
) -> Result<Option<usize>> {
    let mut output = LengthWriter::default();
    match write_statement(&mut output, table, auto_increment) {
        Ok(()) => Ok(Some(output.len)),
        Err(FormatError::Write) => Ok(None),
        Err(FormatError::Allocation) => Err(allocation_error()),
    }
}

pub(super) fn render(
    table: &TableSchema,
    auto_increment: Option<AutoIncrement>,
    length: usize,
) -> Result<String> {
    let mut output = String::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| allocation_error())?;
    match write_statement(&mut output, table, auto_increment) {
        Ok(()) => {}
        Err(FormatError::Write) => {
            return Err(Error::Capacity {
                operation: "formatting SHOW CREATE TABLE output",
            });
        }
        Err(FormatError::Allocation) => return Err(allocation_error()),
    }
    debug_assert_eq!(output.len(), length);
    Ok(output)
}

fn write_statement(
    output: &mut impl fmt::Write,
    table: &TableSchema,
    auto_increment: Option<AutoIncrement>,
) -> std::result::Result<(), FormatError> {
    output
        .write_str("CREATE TABLE ")
        .map_err(|_| FormatError::Write)?;
    write_identifier(output, &table.name)?;
    output.write_str(" (").map_err(|_| FormatError::Write)?;
    let mut foreign_keys = table.foreign_keys.iter().peekable();
    for index in 0..table.columns.len() {
        if index > 0 {
            output.write_str(", ").map_err(|_| FormatError::Write)?;
        }
        let foreign_key = if foreign_keys
            .peek()
            .is_some_and(|foreign_key| foreign_key.column == index)
        {
            foreign_keys.next()
        } else {
            None
        };
        write_column(
            output,
            table,
            index,
            foreign_key,
            auto_increment.is_some_and(|state| state.column == index),
        )?;
    }
    debug_assert!(foreign_keys.next().is_none());

    for program in &table.checks {
        output
            .write_str(", CHECK (")
            .map_err(|_| FormatError::Write)?;
        check::write(output, table, program).map_err(FormatError::from)?;
        output.write_char(')').map_err(|_| FormatError::Write)?;
    }
    output.write_char(')').map_err(|_| FormatError::Write)
}

fn write_column(
    output: &mut impl fmt::Write,
    table: &TableSchema,
    index: usize,
    foreign_key: Option<&ForeignKey>,
    auto_increment: bool,
) -> std::result::Result<(), FormatError> {
    let column = &table.columns[index];
    write_identifier(output, &column.name)?;
    write!(output, " {}", column.data_type).map_err(|_| FormatError::Write)?;
    if !column.nullable {
        output
            .write_str(" NOT NULL")
            .map_err(|_| FormatError::Write)?;
    }
    if table.primary_key == Some(index) {
        output
            .write_str(" PRIMARY KEY")
            .map_err(|_| FormatError::Write)?;
    }
    if auto_increment {
        output
            .write_str(" AUTOINCREMENT")
            .map_err(|_| FormatError::Write)?;
    }
    if table.unique_columns.binary_search(&index).is_ok() {
        output
            .write_str(" UNIQUE")
            .map_err(|_| FormatError::Write)?;
    }
    if let Some(default) = &column.default {
        output
            .write_str(" DEFAULT ")
            .map_err(|_| FormatError::Write)?;
        write_value(output, default)?;
    }
    if let Some(foreign_key) = foreign_key {
        write_foreign_key(output, foreign_key)?;
    }
    Ok(())
}

fn write_foreign_key(
    output: &mut impl fmt::Write,
    foreign_key: &ForeignKey,
) -> std::result::Result<(), FormatError> {
    output
        .write_str(" REFERENCES ")
        .map_err(|_| FormatError::Write)?;
    write_identifier(output, &foreign_key.referenced_table)?;
    output.write_char('(').map_err(|_| FormatError::Write)?;
    write_identifier(output, &foreign_key.referenced_column)?;
    write!(
        output,
        ") ON DELETE {} ON UPDATE {}",
        DeleteAction(foreign_key.on_delete),
        UpdateAction(foreign_key.on_update),
    )
    .map_err(|_| FormatError::Write)
}

pub(super) fn write_identifier(
    output: &mut impl fmt::Write,
    identifier: &str,
) -> std::result::Result<(), FormatError> {
    if is_reserved_identifier(identifier) {
        output.write_char('"').map_err(|_| FormatError::Write)?;
        output
            .write_str(identifier)
            .map_err(|_| FormatError::Write)?;
        output.write_char('"').map_err(|_| FormatError::Write)
    } else {
        output.write_str(identifier).map_err(|_| FormatError::Write)
    }
}

/// Writes `value` as the SQL literal that parses back to it.
///
/// Replayable DDL has to spell literals exactly the way the rest of the crate
/// does, so this defers to the shared renderer instead of keeping a second copy
/// of the quoting and apostrophe-doubling rules.
pub(super) fn write_value(
    output: &mut impl fmt::Write,
    value: &Value,
) -> std::result::Result<(), FormatError> {
    format_value(output, value).map_err(|_| FormatError::Write)
}

#[derive(Clone, Copy)]
pub(super) enum FormatError {
    Write,
    Allocation,
}

impl From<check::CheckFormatError> for FormatError {
    fn from(error: check::CheckFormatError) -> Self {
        match error {
            check::CheckFormatError::Write => Self::Write,
            check::CheckFormatError::Allocation => Self::Allocation,
        }
    }
}

#[derive(Default)]
struct LengthWriter {
    len: usize,
}

impl fmt::Write for LengthWriter {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.len = self.len.checked_add(value.len()).ok_or(fmt::Error)?;
        Ok(())
    }
}

struct DeleteAction(ForeignKeyDeleteAction);

impl fmt::Display for DeleteAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0 {
            ForeignKeyDeleteAction::Restrict => "RESTRICT",
            ForeignKeyDeleteAction::Cascade => "CASCADE",
            ForeignKeyDeleteAction::SetNull => "SET NULL",
        })
    }
}

struct UpdateAction(ForeignKeyUpdateAction);

impl fmt::Display for UpdateAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.0 {
            ForeignKeyUpdateAction::Restrict => "RESTRICT",
            ForeignKeyUpdateAction::Cascade => "CASCADE",
        })
    }
}

const fn allocation_error() -> Error {
    Error::Allocation {
        operation: "formatting SHOW CREATE TABLE output",
    }
}
