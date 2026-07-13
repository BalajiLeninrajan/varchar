//! Physical compilation of semantically resolved predicates into row regexes.

use fancy_regex::{Regex, RegexBuilder};

use super::{ScanPlan, SelectPlan, SelectSource};
use crate::limits::{Limits, check_limit};
use crate::resolve::{LikeAtom, ResolvedPredicate, ResolvedSelect};
use crate::storage::{self, TableSchema};
use crate::{Column, Error, Result};

#[derive(Clone, Debug)]
enum CompiledPredicate {
    Equal { column: usize, encoded: String },
    NotEqual { column: usize, encoded: String },
    Like { column: usize, pattern: String },
    IsNull { column: usize },
    IsNotNull { column: usize },
}

pub(super) fn select(resolved: ResolvedSelect<'_, '_>, limits: &Limits) -> Result<SelectPlan> {
    let ResolvedSelect {
        sources,
        projection,
        joins,
        predicates,
    } = resolved;

    let mut predicates_by_source = Vec::with_capacity(sources.len());
    predicates_by_source.resize_with(sources.len(), Vec::new);
    for resolved in predicates {
        let compiled = compile_predicate(sources[resolved.source], resolved.predicate, limits)?;
        predicates_by_source[resolved.source].push(compiled);
    }

    let mut source_patterns = Vec::with_capacity(sources.len());
    for (schema, predicates) in sources.iter().zip(&predicates_by_source) {
        source_patterns.push(compile_row_pattern(
            &schema.name,
            &schema.columns,
            predicates,
            limits,
        )?);
    }
    let pattern = if source_patterns.len() == 1 {
        source_patterns
            .pop()
            .expect("a resolved SELECT always has a root source")
    } else {
        let mut combined = PatternBuilder::new(limits.max_pattern_bytes);
        combined.push_str("(?:")?;
        for (index, pattern) in source_patterns.iter().enumerate() {
            if index > 0 {
                combined.push_char('|')?;
            }
            combined.push_str(pattern)?;
        }
        combined.push_char(')')?;
        combined.finish()
    };
    let regex = build_regex(&pattern, limits)?;
    let sources = sources
        .into_iter()
        .map(|schema| SelectSource {
            table: schema.name.clone(),
            schema: schema.columns.clone(),
        })
        .collect();

    Ok(SelectPlan {
        pattern,
        regex,
        sources,
        projection,
        joins,
    })
}

pub(super) fn scan<'a>(
    schema: &TableSchema,
    predicates: impl Iterator<Item = Result<ResolvedPredicate<'a>>>,
    limits: &Limits,
) -> Result<ScanPlan> {
    let predicates = compile_predicates(schema, predicates, limits)?;
    let pattern = compile_row_pattern(&schema.name, &schema.columns, &predicates, limits)?;
    // Compile eagerly so public planning never returns an unusable pattern.
    let regex = build_regex(&pattern, limits)?;
    Ok(ScanPlan {
        regex,
        table: schema.name.clone(),
        schema: schema.columns.clone(),
    })
}

fn compile_predicates<'a>(
    schema: &TableSchema,
    predicates: impl Iterator<Item = Result<ResolvedPredicate<'a>>>,
    limits: &Limits,
) -> Result<Vec<CompiledPredicate>> {
    predicates
        .map(|predicate| compile_predicate(schema, predicate?, limits))
        .collect()
}

fn compile_predicate(
    schema: &TableSchema,
    predicate: ResolvedPredicate<'_>,
    limits: &Limits,
) -> Result<CompiledPredicate> {
    match predicate {
        ResolvedPredicate::Equal { column, value } => {
            let encoded = storage::encode_cell(value, &schema.columns[column])?;
            Ok(CompiledPredicate::Equal { column, encoded })
        }
        ResolvedPredicate::NotEqual { column, value } => {
            let encoded = storage::encode_cell(value, &schema.columns[column])?;
            Ok(CompiledPredicate::NotEqual { column, encoded })
        }
        ResolvedPredicate::Like { column, atoms } => Ok(CompiledPredicate::Like {
            column,
            pattern: compile_like_pattern(&atoms, limits)?,
        }),
        ResolvedPredicate::IsNull { column } => Ok(CompiledPredicate::IsNull { column }),
        ResolvedPredicate::IsNotNull { column } => Ok(CompiledPredicate::IsNotNull { column }),
    }
}

fn compile_row_pattern(
    table: &str,
    schema: &[Column],
    predicates: &[CompiledPredicate],
    limits: &Limits,
) -> Result<String> {
    check_limit(predicates.len(), limits.max_predicates, "WHERE predicates")?;

    let mut pattern = PatternBuilder::new(limits.max_pattern_bytes);
    pattern.push_str(&storage::row_prefix_pattern(table))?;
    for predicate in predicates {
        let column_index = predicate.column();
        pattern.push_str("(?=")?;
        for column in &schema[..column_index] {
            pattern.push_str(&storage::cell_pattern(column, true))?;
            pattern.push_str(r"\|")?;
        }
        match predicate {
            CompiledPredicate::Equal { encoded, .. } => {
                pattern.push_str(&regex::escape(encoded))?;
            }
            CompiledPredicate::NotEqual { encoded, .. } => {
                pattern.push_str("(?!")?;
                pattern.push_str(&regex::escape(encoded))?;
                pattern.push_str(storage::cell_boundary_pattern(column_index, schema.len()))?;
                pattern.push_char(')')?;
                pattern.push_str(&storage::cell_pattern(&schema[column_index], false))?;
            }
            CompiledPredicate::Like {
                pattern: like_pattern,
                ..
            } => pattern.push_str(like_pattern)?,
            CompiledPredicate::IsNull { .. } => pattern.push_char('N')?,
            CompiledPredicate::IsNotNull { .. } => {
                pattern.push_str(&storage::cell_pattern(&schema[column_index], false))?;
            }
        }
        pattern.push_str(storage::cell_boundary_pattern(column_index, schema.len()))?;
        pattern.push_char(')')?;
    }

    for (index, column) in schema.iter().enumerate() {
        if index > 0 {
            pattern.push_str(r"\|")?;
        }
        pattern.push_str(&storage::cell_pattern(column, true))?;
    }
    pattern.push_char(';')?;
    Ok(pattern.finish())
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
                    resource: "generated regex bytes",
                    limit: self.limit,
                })?;
        check_limit(new_len, self.limit, "generated regex bytes")?;
        self.pattern
            .try_reserve(fragment.len())
            .map_err(|_| Error::ResourceLimit {
                resource: "generated regex bytes",
                limit: self.limit,
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

impl CompiledPredicate {
    const fn column(&self) -> usize {
        match self {
            Self::Equal { column, .. }
            | Self::NotEqual { column, .. }
            | Self::Like { column, .. }
            | Self::IsNull { column }
            | Self::IsNotNull { column } => *column,
        }
    }
}

fn compile_like_pattern(atoms: &[LikeAtom], limits: &Limits) -> Result<String> {
    let mut result = PatternBuilder::new(limits.max_pattern_bytes);
    result.push_str("T")?;
    let mut previous_was_many = false;
    for atom in atoms {
        match atom {
            LikeAtom::AnySequence => {
                if !previous_was_many {
                    result.push_str(storage::text_unit_pattern())?;
                    result.push_char('*')?;
                    previous_was_many = true;
                }
            }
            LikeAtom::AnyScalar => {
                result.push_str(storage::text_unit_pattern())?;
                previous_was_many = false;
            }
            LikeAtom::Literal(literal) => {
                push_encoded_text_literal(&mut result, *literal)?;
                previous_was_many = false;
            }
        }
    }
    Ok(result.finish())
}

fn push_encoded_text_literal(result: &mut PatternBuilder, character: char) -> Result<()> {
    result.push_str(&storage::encoded_text_literal_pattern(character))
}

pub(super) fn build_regex(pattern: &str, limits: &Limits) -> Result<Regex> {
    let mut builder = RegexBuilder::new(pattern);
    builder
        .backtrack_limit(limits.regex_backtrack_limit)
        .delegate_size_limit(limits.max_pattern_bytes);
    builder
        .build()
        .map_err(|error| Error::RegexCompile(error.to_string()))
}
