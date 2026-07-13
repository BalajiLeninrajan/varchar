//! Complete V2 row-scan regex construction.

use super::RowLayout;
use super::encode::encode_cell;
use super::format::{ROW_PREFIX, encode_text_into};
use crate::limits::check_limit;
use crate::{Column, DataType, Error, Resource, Result, Value};

const TEXT_UNIT_PATTERN: &str = r"(?:%[0-9A-F]{6}|[^%|;~])";

/// Physical atoms for matching one encoded TEXT cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextPatternAtom {
    AnySequence,
    AnyScalar,
    Literal(char),
}

/// One typed cell matcher lowered to the V2 representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RowPredicatePattern {
    Equal { column: usize, encoded: String },
    NotEqual { column: usize, encoded: String },
    Text { column: usize, pattern: String },
    IsNull { column: usize },
    IsNotNull { column: usize },
}

impl RowPredicatePattern {
    pub(crate) fn equal(column: usize, value: &Value, definition: &Column) -> Result<Self> {
        Ok(Self::Equal {
            column,
            encoded: encode_cell(value, definition)?,
        })
    }

    pub(crate) fn not_equal(column: usize, value: &Value, definition: &Column) -> Result<Self> {
        Ok(Self::NotEqual {
            column,
            encoded: encode_cell(value, definition)?,
        })
    }

    pub(crate) fn text(
        column: usize,
        atoms: impl IntoIterator<Item = TextPatternAtom>,
        max_pattern_bytes: usize,
    ) -> Result<Self> {
        let mut pattern = PatternBuilder::new(max_pattern_bytes);
        pattern.push_str("T")?;
        let mut previous_was_many = false;
        for atom in atoms {
            match atom {
                TextPatternAtom::AnySequence => {
                    if !previous_was_many {
                        pattern.push_str(TEXT_UNIT_PATTERN)?;
                        pattern.push_char('*')?;
                        previous_was_many = true;
                    }
                }
                TextPatternAtom::AnyScalar => {
                    pattern.push_str(TEXT_UNIT_PATTERN)?;
                    previous_was_many = false;
                }
                TextPatternAtom::Literal(character) => {
                    pattern.push_str(&encoded_text_literal_pattern(character))?;
                    previous_was_many = false;
                }
            }
        }
        Ok(Self::Text {
            column,
            pattern: pattern.finish(),
        })
    }

    pub(crate) const fn is_null(column: usize) -> Self {
        Self::IsNull { column }
    }

    pub(crate) const fn is_not_null(column: usize) -> Self {
        Self::IsNotNull { column }
    }

    const fn column(&self) -> usize {
        match self {
            Self::Equal { column, .. }
            | Self::NotEqual { column, .. }
            | Self::Text { column, .. }
            | Self::IsNull { column }
            | Self::IsNotNull { column } => *column,
        }
    }
}

/// Build one complete row pattern, from its V2 tag through its terminator.
pub(crate) fn row_scan_pattern(
    layout: RowLayout<'_>,
    predicates: &[RowPredicatePattern],
    max_pattern_bytes: usize,
) -> Result<String> {
    let mut pattern = PatternBuilder::new(max_pattern_bytes);
    // `regex::escape` also escapes `~`; retaining its historical unescaped
    // spelling keeps the public explanation pattern byte-for-byte stable.
    let row_prefix = regex::escape(ROW_PREFIX).replace(r"\~", "~");
    pattern.push_str(&row_prefix)?;
    pattern.push_str(&regex::escape(layout.table))?;
    pattern.push_str(r"\|")?;
    for predicate in predicates {
        let column_index = predicate.column();
        pattern.push_str("(?=")?;
        for column in &layout.columns[..column_index] {
            pattern.push_str(&cell_pattern(column, true))?;
            pattern.push_str(r"\|")?;
        }
        match predicate {
            RowPredicatePattern::Equal { encoded, .. } => {
                pattern.push_str(&regex::escape(encoded))?;
            }
            RowPredicatePattern::NotEqual { encoded, .. } => {
                pattern.push_str("(?!")?;
                pattern.push_str(&regex::escape(encoded))?;
                pattern.push_str(cell_boundary_pattern(column_index, layout.columns.len()))?;
                pattern.push_char(')')?;
                pattern.push_str(&cell_pattern(&layout.columns[column_index], false))?;
            }
            RowPredicatePattern::Text {
                pattern: text_pattern,
                ..
            } => pattern.push_str(text_pattern)?,
            RowPredicatePattern::IsNull { .. } => pattern.push_char('N')?,
            RowPredicatePattern::IsNotNull { .. } => {
                pattern.push_str(&cell_pattern(&layout.columns[column_index], false))?;
            }
        }
        pattern.push_str(cell_boundary_pattern(column_index, layout.columns.len()))?;
        pattern.push_char(')')?;
    }

    for (index, column) in layout.columns.iter().enumerate() {
        if index > 0 {
            pattern.push_str(r"\|")?;
        }
        pattern.push_str(&cell_pattern(column, true))?;
    }
    pattern.push_char(';')?;
    Ok(pattern.finish())
}

fn cell_boundary_pattern(column: usize, column_count: usize) -> &'static str {
    if column + 1 == column_count {
        ";"
    } else {
        r"\|"
    }
}

fn cell_pattern(column: &Column, include_null: bool) -> String {
    let typed = match column.data_type {
        DataType::Text => format!("T{TEXT_UNIT_PATTERN}*"),
        DataType::Integer => String::from(r"I(?:0|-?[1-9][0-9]*)"),
        DataType::Boolean => String::from(r"B[01]"),
    };
    if include_null && column.nullable {
        format!("(?:N|{typed})")
    } else {
        typed
    }
}

fn encoded_text_literal_pattern(character: char) -> String {
    let mut encoded = String::new();
    encode_text_into(&character.to_string(), &mut encoded);
    regex::escape(&encoded)
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
        let new_len =
            self.pattern
                .len()
                .checked_add(fragment.len())
                .ok_or(Error::ResourceLimit {
                    resource: Resource::GeneratedRegexBytes,
                    limit: self.limit,
                })?;
        check_limit(new_len, self.limit, Resource::GeneratedRegexBytes)?;
        self.pattern
            .try_reserve(fragment.len())
            .map_err(|_| Error::Allocation {
                operation: "building a generated regex",
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
}

#[cfg(test)]
mod tests {
    use fancy_regex::Regex;

    use super::{RowPredicatePattern, TextPatternAtom, row_scan_pattern};
    use crate::storage::RowLayout;
    use crate::{Column, DataType, Value};

    const MAX_PATTERN_BYTES: usize = 16 * 1024;

    fn matches(pattern: &str, row: &str) -> bool {
        Regex::new(&format!("^(?:{pattern})$"))
            .expect("test pattern compiles")
            .is_match(row)
            .expect("test pattern executes")
    }

    fn integer_column(name: &str) -> Column {
        Column {
            name: name.to_owned(),
            data_type: DataType::Integer,
            nullable: false,
        }
    }

    #[test]
    fn complete_patterns_route_exact_tables_and_row_boundaries() {
        let columns = [integer_column("id")];
        let pattern = row_scan_pattern(
            RowLayout {
                table: "user",
                columns: &columns,
            },
            &[],
            MAX_PATTERN_BYTES,
        )
        .expect("row pattern");

        assert!(matches(&pattern, "~R|user|I1;"));
        assert!(!matches(&pattern, "~R|users|I1;"));
        assert!(!matches(&pattern, "~R|user|I1;~R|user|I2;"));
    }

    #[test]
    fn escaped_like_atoms_are_lowered_inside_storage() {
        let columns = [Column {
            name: String::from("body"),
            data_type: DataType::Text,
            nullable: true,
        }];
        let predicate = RowPredicatePattern::text(
            0,
            [
                TextPatternAtom::Literal('|'),
                TextPatternAtom::AnySequence,
                TextPatternAtom::Literal(';'),
            ],
            MAX_PATTERN_BYTES,
        )
        .expect("LIKE pattern");
        let pattern = row_scan_pattern(
            RowLayout {
                table: "notes",
                columns: &columns,
            },
            &[predicate],
            MAX_PATTERN_BYTES,
        )
        .expect("row pattern");

        assert!(matches(&pattern, "~R|notes|T%00007Cmiddle%00003B;"));
        assert!(!matches(&pattern, "~R|notes|Tmiddle;"));
        assert!(!matches(&pattern, "~R|notes|N;"));
    }

    #[test]
    fn null_and_typed_value_predicates_use_complete_cell_boundaries() {
        let columns = [
            integer_column("id"),
            Column {
                name: String::from("note"),
                data_type: DataType::Text,
                nullable: true,
            },
        ];
        let predicates = [
            RowPredicatePattern::equal(0, &Value::Integer(7), &columns[0])
                .expect("integer predicate"),
            RowPredicatePattern::is_not_null(1),
        ];
        let value_pattern = row_scan_pattern(
            RowLayout {
                table: "items",
                columns: &columns,
            },
            &predicates,
            MAX_PATTERN_BYTES,
        )
        .expect("value pattern");
        let null_pattern = row_scan_pattern(
            RowLayout {
                table: "items",
                columns: &columns,
            },
            &[RowPredicatePattern::is_null(1)],
            MAX_PATTERN_BYTES,
        )
        .expect("NULL pattern");

        assert!(matches(&value_pattern, "~R|items|I7|Tkept;"));
        assert!(!matches(&value_pattern, "~R|items|I70|Tkept;"));
        assert!(!matches(&value_pattern, "~R|items|I7|N;"));
        assert!(matches(&null_pattern, "~R|items|I70|N;"));
        assert!(!matches(&null_pattern, "~R|items|I70|Tvalue;"));
    }
}
