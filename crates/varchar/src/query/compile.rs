//! Physical compilation of semantically resolved predicates into row regexes.

use fancy_regex::{Regex, RegexBuilder};

use super::{ScanPlan, SelectPlan};
use crate::limits::{Limits, check_limit};
use crate::resolve::{LikeAtom, ResolvedPredicate, ResolvedSelect};
use crate::storage::{self, RowPredicatePattern, TableSchema, TextPatternAtom};
use crate::{Error, Result};

pub(super) fn select<'catalog>(
    resolved: ResolvedSelect<'catalog, '_>,
    limits: &Limits,
) -> Result<SelectPlan<'catalog>> {
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

    let pattern = if sources.len() == 1 {
        storage::row_scan_pattern(
            sources[0].row_layout(),
            &predicates_by_source[0],
            limits.max_pattern_bytes,
        )?
    } else {
        alternate_source_patterns(
            sources
                .iter()
                .zip(&predicates_by_source)
                .map(|(schema, predicates)| {
                    storage::row_scan_pattern(
                        schema.row_layout(),
                        predicates,
                        limits.max_pattern_bytes,
                    )
                }),
            limits.max_pattern_bytes,
        )?
    };
    let regex = build_regex(&pattern, limits)?;
    Ok(SelectPlan {
        pattern,
        regex,
        sources,
        projection,
        joins,
    })
}

pub(super) fn scan<'catalog, 'statement>(
    schema: &'catalog TableSchema,
    predicates: impl Iterator<Item = Result<ResolvedPredicate<'statement>>>,
    limits: &Limits,
) -> Result<ScanPlan<'catalog>> {
    let predicates = compile_predicates(schema, predicates, limits)?;
    let pattern =
        storage::row_scan_pattern(schema.row_layout(), &predicates, limits.max_pattern_bytes)?;
    // Compile eagerly so public planning never returns an unusable pattern.
    let regex = build_regex(&pattern, limits)?;
    Ok(ScanPlan { regex, schema })
}

fn compile_predicates<'a>(
    schema: &TableSchema,
    predicates: impl Iterator<Item = Result<ResolvedPredicate<'a>>>,
    limits: &Limits,
) -> Result<Vec<RowPredicatePattern>> {
    predicates
        .map(|predicate| compile_predicate(schema, predicate?, limits))
        .collect()
}

fn compile_predicate(
    schema: &TableSchema,
    predicate: ResolvedPredicate<'_>,
    limits: &Limits,
) -> Result<RowPredicatePattern> {
    match predicate {
        ResolvedPredicate::Equal { column, value } => {
            RowPredicatePattern::equal(column, value, &schema.columns[column])
        }
        ResolvedPredicate::NotEqual { column, value } => {
            RowPredicatePattern::not_equal(column, value, &schema.columns[column])
        }
        ResolvedPredicate::Like { column, atoms } => RowPredicatePattern::text(
            column,
            atoms.into_iter().map(|atom| match atom {
                LikeAtom::AnySequence => TextPatternAtom::AnySequence,
                LikeAtom::AnyScalar => TextPatternAtom::AnyScalar,
                LikeAtom::Literal(character) => TextPatternAtom::Literal(character),
            }),
            limits.max_pattern_bytes,
        ),
        ResolvedPredicate::IsNull { column } => Ok(RowPredicatePattern::is_null(column)),
        ResolvedPredicate::IsNotNull { column } => Ok(RowPredicatePattern::is_not_null(column)),
    }
}

fn alternate_source_patterns(
    patterns: impl IntoIterator<Item = Result<String>>,
    max_pattern_bytes: usize,
) -> Result<String> {
    check_limit(3, max_pattern_bytes, "generated regex bytes")?;
    let mut combined = String::new();
    combined
        .try_reserve_exact(3)
        .map_err(|_| pattern_limit_error(max_pattern_bytes))?;
    combined.push_str("(?:");
    for (index, pattern) in patterns.into_iter().enumerate() {
        let pattern = pattern?;
        let separator_bytes = usize::from(index > 0);
        let additional = separator_bytes
            .checked_add(pattern.len())
            .ok_or_else(|| pattern_limit_error(max_pattern_bytes))?;
        let next_len = combined
            .len()
            .checked_add(additional)
            .ok_or_else(|| pattern_limit_error(max_pattern_bytes))?;
        check_limit(next_len, max_pattern_bytes, "generated regex bytes")?;
        combined
            .try_reserve(additional)
            .map_err(|_| pattern_limit_error(max_pattern_bytes))?;
        if index > 0 {
            combined.push('|');
        }
        combined.push_str(&pattern);
    }
    let final_len = combined
        .len()
        .checked_add(1)
        .ok_or_else(|| pattern_limit_error(max_pattern_bytes))?;
    check_limit(final_len, max_pattern_bytes, "generated regex bytes")?;
    combined
        .try_reserve(1)
        .map_err(|_| pattern_limit_error(max_pattern_bytes))?;
    combined.push(')');
    Ok(combined)
}

fn pattern_limit_error(limit: usize) -> Error {
    Error::ResourceLimit {
        resource: "generated regex bytes",
        limit,
    }
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
