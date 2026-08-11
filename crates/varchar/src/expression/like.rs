//! Validation and decoded matching for SQL `LIKE` patterns.

use crate::{Error, Resource, Result};

/// Logical atoms in one validated SQL `LIKE` pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LikeAtom {
    AnySequence,
    AnyScalar,
    Literal(char),
}

pub(crate) fn compile_pattern(pattern: &str) -> Result<Vec<LikeAtom>> {
    let mut atoms = Vec::new();
    atoms
        .try_reserve_exact(pattern.chars().count())
        .map_err(|_| Error::Allocation {
            operation: "reserving a resolved LIKE pattern",
        })?;

    let mut characters = pattern.chars();
    while let Some(character) = characters.next() {
        match character {
            '%' => atoms.push(LikeAtom::AnySequence),
            '_' => atoms.push(LikeAtom::AnyScalar),
            '\\' => {
                let Some(escaped) = characters.next() else {
                    return Err(Error::Type(String::from(
                        "LIKE pattern ends with an incomplete escape",
                    )));
                };
                if !matches!(escaped, '%' | '_' | '\\') {
                    return Err(Error::Type(format!(
                        "LIKE pattern contains unsupported escape \\{escaped}"
                    )));
                }
                atoms.push(LikeAtom::Literal(escaped));
            }
            literal => atoms.push(LikeAtom::Literal(literal)),
        }
    }
    Ok(atoms)
}

/// Atom comparisons one search may make per byte of the value before its work
/// stops counting as ordinary forward scanning.
///
/// A segment compares its first atom at each candidate start, and a short run
/// of further atoms whenever a start looks promising. That work is proportional
/// to the value, and reading the value is what the scan pattern does too, so
/// charging it would meter reading rather than backtracking. Only comparisons
/// beyond this allowance are charged.
const SCAN_ALLOWANCE: usize = 4;

/// Wildcard-matching work shared by every `LIKE` search in one statement.
///
/// The compiled scan regex spends a single [`Limits::regex_backtrack_limit`]
/// budget for the whole pattern, so a residual `LIKE` must not be handed a
/// fresh budget per row or per predicate: a statement carrying many residual
/// `LIKE` leaves would otherwise multiply the bound by their product. One
/// counter therefore lives in the evaluator and outlives every row it visits.
///
/// [`Limits::regex_backtrack_limit`]: crate::Limits::regex_backtrack_limit
pub(super) struct LikeWork {
    used: usize,
    limit: usize,
}

impl LikeWork {
    pub(super) const fn new(limit: usize) -> Self {
        Self { used: 0, limit }
    }

    fn charge(&mut self) -> Result<()> {
        let used = self.used.checked_add(1).ok_or_else(|| self.error())?;
        if used > self.limit {
            return Err(self.error());
        }
        self.used = used;
        Ok(())
    }

    const fn error(&self) -> Error {
        Error::ResourceLimit {
            resource: Resource::RegexBacktracking,
            limit: self.limit,
        }
    }
}

/// One `LIKE` search against the statement-wide budget.
///
/// `free` is the forward-scanning allowance this search may spend before it
/// starts drawing on the shared budget. It is deliberately independent of the
/// pattern shape: a pattern cannot enlarge its own allowance by adding
/// segments, so no shape buys unmetered quadratic work.
struct Search<'work> {
    work: &'work mut LikeWork,
    free: usize,
}

impl Search<'_> {
    fn charge(&mut self) -> Result<()> {
        if let Some(remaining) = self.free.checked_sub(1) {
            self.free = remaining;
            return Ok(());
        }
        self.work.charge()
    }
}

/// Match decoded text without recursive wildcard backtracking or allocation.
///
/// The pattern is matched segment by segment: the run of atoms before the first
/// `%` is anchored at the start, the run after the last `%` is anchored at the
/// end, and each interior run is placed at its earliest possible offset. That
/// keeps the common shapes — a bare `%suffix`, `prefix%`, or `%infix%` — linear
/// in the value instead of restarting a literal run at every scalar.
///
/// An interior run can still be retried at every candidate start, so work grows
/// with the product of the two lengths for repetitive text. Text is bounded only
/// by `max_database_bytes` while the pattern is bounded by `max_sql_bytes`, so
/// one row could otherwise dominate a query. Work beyond a forward scan is
/// charged to `work`, the budget shared by every `LIKE` in the statement, and
/// exhausting it returns [`Error::ResourceLimit`] for
/// [`Resource::RegexBacktracking`].
pub(super) fn matches_charged(
    value: &str,
    atoms: &[LikeAtom],
    work: &mut LikeWork,
) -> Result<bool> {
    let free = value
        .len()
        .saturating_mul(SCAN_ALLOWANCE)
        .saturating_add(atoms.len());
    matches_inner(value, atoms, &mut Search { work, free })
}

fn matches_inner(value: &str, atoms: &[LikeAtom], search: &mut Search<'_>) -> Result<bool> {
    let mut segments = atoms
        .split(|atom| matches!(atom, LikeAtom::AnySequence))
        .peekable();
    let leading = segments
        .next()
        .expect("splitting a slice always yields one segment");

    // Atoms before the first `%` are anchored at the start of the value.
    let Some(mut cursor) = match_at(value, 0, leading, search)? else {
        return Ok(false);
    };

    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            // Atoms after the last `%` are anchored at the end of the value.
            return match_suffix(value, cursor, segment, search);
        }
        // Placing an interior segment as early as possible leaves the most room
        // for the segments after it, so the greedy placement never rejects a
        // value the pattern could have matched.
        let Some(placed) = find_from(value, cursor, segment, search)? else {
            return Ok(false);
        };
        cursor = placed;
    }

    // A pattern without `%` matches only if its atoms consumed the whole value.
    Ok(cursor == value.len())
}

/// Match one wildcard-free segment at `offset`, returning the offset after it.
fn match_at(
    value: &str,
    mut offset: usize,
    segment: &[LikeAtom],
    search: &mut Search<'_>,
) -> Result<Option<usize>> {
    for atom in segment {
        search.charge()?;
        let Some((scalar, next)) = next_scalar(value, offset) else {
            return Ok(None);
        };
        let accepted = match atom {
            LikeAtom::Literal(expected) => *expected == scalar,
            // Splitting on `AnySequence` leaves only atoms that consume exactly
            // one scalar, so `_` and an unreachable `%` behave alike here.
            LikeAtom::AnyScalar | LikeAtom::AnySequence => true,
        };
        if !accepted {
            return Ok(None);
        }
        offset = next;
    }
    Ok(Some(offset))
}

/// Place `segment` at its earliest match at or after `cursor`.
fn find_from(
    value: &str,
    mut cursor: usize,
    segment: &[LikeAtom],
    search: &mut Search<'_>,
) -> Result<Option<usize>> {
    loop {
        if let Some(LikeAtom::Literal(first)) = segment.first() {
            // Skipping to the next occurrence of the leading literal is a byte
            // scan, not a retry, so it stays outside the charged work.
            let Some(found) = value.get(cursor..).and_then(|rest| rest.find(*first)) else {
                return Ok(None);
            };
            cursor += found;
        }
        if let Some(end) = match_at(value, cursor, segment, search)? {
            return Ok(Some(end));
        }
        let Some((_, next)) = next_scalar(value, cursor) else {
            return Ok(None);
        };
        cursor = next;
    }
}

/// Match `segment` against the end of the value, no earlier than `cursor`.
fn match_suffix(
    value: &str,
    cursor: usize,
    segment: &[LikeAtom],
    search: &mut Search<'_>,
) -> Result<bool> {
    let mut start = value.len();
    for _ in segment {
        search.charge()?;
        let Some(previous) = previous_scalar(value, start) else {
            return Ok(false);
        };
        start = previous;
    }
    if start < cursor {
        return Ok(false);
    }
    Ok(match_at(value, start, segment, search)? == Some(value.len()))
}

fn next_scalar(value: &str, offset: usize) -> Option<(char, usize)> {
    let character = value.get(offset..)?.chars().next()?;
    Some((character, offset + character.len_utf8()))
}

fn previous_scalar(value: &str, offset: usize) -> Option<usize> {
    let character = value.get(..offset)?.chars().next_back()?;
    Some(offset - character.len_utf8())
}
