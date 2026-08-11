# Architecture

## Workspace layout

The Cargo workspace has two parts:

- `varchar` is the platform-neutral core. It owns the database string and implements storage validation, SQL parsing, type checking, regex planning, and query execution.
- `varchar-cli` builds the native `varchar` binary. It owns files, atomic replacement, terminal input, and output formatting.

The core has no filesystem or terminal API. Parsed schemas, syntax trees, compiled regexes, and result rows may exist temporarily, but the one string remains the only authoritative database state.

## Query execution

Every supported `SELECT` compiles the scans for all participating tables into one regex—an alternation for joins. Safe predicate leaves from a top-level conjunction become exact regex prefilters; Rust evaluates the remaining Boolean expression against decoded values. For a join, source-local residuals run before rows are retained, `ON` conditions run during left-to-right nested loops, and cross-source residuals run afterward. `EXPLAIN REGEX` exposes the generated scan prefilter, which may represent only part of the `WHERE` expression, so the trick stays visible. `SelectExplanation::pattern_is_exact` reports which case a caller has: `true` means the pattern expresses all row filtering and selects exactly the rows the query retains, `false` means the pattern is a prefilter that over-selects and Rust-side evaluation decides the rest. A join is never exact, because its pattern is an alternation over whole source rows and `ON` conditions run in Rust. Clauses that never eliminate source rows—projection, and any ordering or pagination the dialect supports—are not represented by the pattern either, and they do not make the flag `false`.

## Mutation execution

`UPDATE` and `DELETE` also compile one single-table scan. They evaluate it once against the original validated string, freeze each matching row's original byte range and decoded values, then build effective replacements and apply the physical edits in source order. The direct affected-row count is fixed before any edit, and complete candidate validation remains the final constraint backstop.

Every mutation builds and validates a candidate string before replacing the old state, so a failed mutation leaves the authoritative string byte-for-byte unchanged.
