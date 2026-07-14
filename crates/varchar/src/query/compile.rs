//! Physical compilation of semantically resolved predicates into row regexes.

use fancy_regex::{Regex, RegexBuilder};

use super::{ScanPlan, SelectPlan, pattern};
use crate::limits::Limits;
use crate::resolve::{ResolvedPredicate, ResolvedSelect};
use crate::storage::TableSchema;
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

    let mut predicates_by_source = Vec::new();
    predicates_by_source
        .try_reserve_exact(sources.len())
        .map_err(|_| Error::allocation("reserving query predicate buckets"))?;
    predicates_by_source.resize_with(sources.len(), Vec::new);
    for resolved in predicates {
        predicates_by_source[resolved.source].push(resolved.predicate);
    }

    let pattern = if sources.len() == 1 {
        pattern::row_scan_pattern(
            &sources[0].name,
            &sources[0].columns,
            &predicates_by_source[0],
            limits.max_pattern_bytes,
        )?
    } else {
        pattern::alternate_source_patterns(
            sources
                .iter()
                .zip(&predicates_by_source)
                .map(|(schema, predicates)| {
                    pattern::row_scan_pattern(
                        &schema.name,
                        &schema.columns,
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

pub(super) fn scan<'statement>(
    schema: &TableSchema,
    predicates: impl Iterator<Item = Result<ResolvedPredicate<'statement>>>,
    limits: &Limits,
) -> Result<ScanPlan> {
    let predicates = predicates.collect::<Result<Vec<_>>>()?;
    let pattern = pattern::row_scan_pattern(
        &schema.name,
        &schema.columns,
        &predicates,
        limits.max_pattern_bytes,
    )?;
    // Compile eagerly so public planning never returns an unusable pattern.
    let regex = build_regex(&pattern, limits)?;
    Ok(ScanPlan {
        regex,
        table: schema.name.clone(),
        schema: schema.columns.clone(),
    })
}

fn build_regex(pattern: &str, limits: &Limits) -> Result<Regex> {
    let mut builder = RegexBuilder::new(pattern);
    builder
        .backtrack_limit(limits.regex_backtrack_limit)
        .delegate_size_limit(limits.max_pattern_bytes);
    builder
        .build()
        .map_err(|error| Error::regex_compile(error.to_string()))
}
