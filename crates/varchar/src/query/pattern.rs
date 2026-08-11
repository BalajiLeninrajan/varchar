//! Complete row-scan regex construction for resolved query predicates.

use crate::limits::check_limit;
use crate::resolve::{LikeAtom, ResolvedPredicate};
use crate::storage::{ROW_PREFIX, encode_cell, encode_text_into};
use crate::{DataType, Error, Resource, Result, SchemaColumn};

const TEXT_UNIT_PATTERN: &str = r"(?:%[0-9A-F]{6}|[^%|;~])";

/// Build one complete row pattern, from its storage tag through its terminator.
pub(super) fn row_scan_pattern(
    table: &str,
    columns: &[SchemaColumn],
    predicates: &[ResolvedPredicate<'_>],
    max_pattern_bytes: usize,
) -> Result<String> {
    let mut pattern = PatternBuilder::new(max_pattern_bytes);
    // `regex::escape` also escapes `~`; retain the established public spelling.
    let row_prefix = regex::escape(ROW_PREFIX).replace(r"\~", "~");
    pattern.push_str(&row_prefix)?;
    pattern.push_str(&regex::escape(table))?;
    pattern.push_str(r"\|")?;

    for predicate in predicates {
        let column_index = predicate_column(predicate);
        if column_index >= columns.len() {
            return Err(Error::Schema(format!(
                "predicate column index {column_index} is outside table {table:?}"
            )));
        }

        pattern.push_str("(?=")?;
        for column in &columns[..column_index] {
            push_cell_pattern(&mut pattern, column, true)?;
            pattern.push_str(r"\|")?;
        }

        match predicate {
            ResolvedPredicate::Equal { value, .. } => {
                let encoded = encode_cell(value, &columns[column_index])?;
                pattern.push_str(&regex::escape(&encoded))?;
            }
            ResolvedPredicate::NotEqual { value, .. } => {
                let encoded = encode_cell(value, &columns[column_index])?;
                pattern.push_str("(?!")?;
                pattern.push_str(&regex::escape(&encoded))?;
                pattern.push_str(cell_boundary_pattern(column_index, columns.len()))?;
                pattern.push_char(')')?;
                push_cell_pattern(&mut pattern, &columns[column_index], false)?;
            }
            ResolvedPredicate::Like { atoms, .. } => {
                push_like_pattern(&mut pattern, atoms)?;
            }
            ResolvedPredicate::IsNull { .. } => pattern.push_char('N')?,
            ResolvedPredicate::IsNotNull { .. } => {
                push_cell_pattern(&mut pattern, &columns[column_index], false)?;
            }
            ResolvedPredicate::LessThan { .. }
            | ResolvedPredicate::LessThanOrEqual { .. }
            | ResolvedPredicate::GreaterThan { .. }
            | ResolvedPredicate::GreaterThanOrEqual { .. }
            | ResolvedPredicate::In { .. } => {
                return Err(Error::Capacity {
                    operation: "building a row pattern from a residual predicate",
                });
            }
        }
        pattern.push_str(cell_boundary_pattern(column_index, columns.len()))?;
        pattern.push_char(')')?;
    }

    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            pattern.push_str(r"\|")?;
        }
        push_cell_pattern(&mut pattern, column, true)?;
    }
    pattern.push_char(';')?;
    Ok(pattern.finish())
}

/// Combine complete row patterns so one scan can gather every joined source.
pub(super) fn alternate_source_patterns(
    patterns: impl IntoIterator<Item = Result<String>>,
    max_pattern_bytes: usize,
) -> Result<String> {
    let mut combined = PatternBuilder::new(max_pattern_bytes);
    combined.push_str("(?:")?;
    for (index, pattern) in patterns.into_iter().enumerate() {
        if index > 0 {
            combined.push_char('|')?;
        }
        combined.push_str(&pattern?)?;
    }
    combined.push_char(')')?;
    Ok(combined.finish())
}

const fn predicate_column(predicate: &ResolvedPredicate<'_>) -> usize {
    predicate.column().column
}

fn push_like_pattern(pattern: &mut PatternBuilder, atoms: &[LikeAtom]) -> Result<()> {
    pattern.push_char('T')?;
    let mut previous_was_many = false;
    for atom in atoms {
        match atom {
            LikeAtom::AnySequence => {
                if !previous_was_many {
                    pattern.push_str(TEXT_UNIT_PATTERN)?;
                    pattern.push_char('*')?;
                    previous_was_many = true;
                }
            }
            LikeAtom::AnyScalar => {
                pattern.push_str(TEXT_UNIT_PATTERN)?;
                previous_was_many = false;
            }
            LikeAtom::Literal(character) => {
                let mut encoded = String::new();
                let mut bytes = [0_u8; 4];
                encode_text_into(character.encode_utf8(&mut bytes), &mut encoded);
                pattern.push_str(&regex::escape(&encoded))?;
                previous_was_many = false;
            }
        }
    }
    Ok(())
}

fn push_cell_pattern(
    pattern: &mut PatternBuilder,
    column: &SchemaColumn,
    include_null: bool,
) -> Result<()> {
    let includes_null = include_null && column.nullable;
    if includes_null {
        pattern.push_str("(?:N|")?;
    }
    match column.data_type {
        DataType::Text => {
            pattern.push_char('T')?;
            pattern.push_str(TEXT_UNIT_PATTERN)?;
            pattern.push_char('*')?;
        }
        DataType::Integer => pattern.push_str(r"I(?:0|-?[1-9][0-9]*)")?,
        DataType::Boolean => pattern.push_str(r"B[01]")?,
    }
    if includes_null {
        pattern.push_char(')')?;
    }
    Ok(())
}

fn cell_boundary_pattern(column: usize, column_count: usize) -> &'static str {
    if column + 1 == column_count {
        ";"
    } else {
        r"\|"
    }
}

struct PatternBuilder {
    pattern: String,
    limit: usize,
}

impl PatternBuilder {
    fn new(limit: usize) -> Self {
        Self {
            pattern: String::new(),
            limit,
        }
    }

    fn push_str(&mut self, fragment: &str) -> Result<()> {
        let new_len = self
            .pattern
            .len()
            .checked_add(fragment.len())
            .ok_or_else(|| self.limit_error())?;
        check_limit(new_len, self.limit, Resource::GeneratedRegexBytes)?;
        self.pattern
            .try_reserve(fragment.len())
            .map_err(|_| Error::Allocation {
                operation: "growing a query row pattern",
            })?;
        self.pattern.push_str(fragment);
        Ok(())
    }

    fn push_char(&mut self, character: char) -> Result<()> {
        let mut encoded = [0_u8; 4];
        self.push_str(character.encode_utf8(&mut encoded))
    }

    fn finish(self) -> String {
        self.pattern
    }

    const fn limit_error(&self) -> Error {
        Error::ResourceLimit {
            resource: Resource::GeneratedRegexBytes,
            limit: self.limit,
        }
    }
}

#[cfg(test)]
mod tests;
