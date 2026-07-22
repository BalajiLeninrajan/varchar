//! Query outcome and row-set rendering.

use std::collections::BTreeMap;
use std::io::{self, Write};

use varchar::{Outcome, RowSet, Value};

pub(crate) fn print_outcome(outcome: &Outcome) -> io::Result<()> {
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
