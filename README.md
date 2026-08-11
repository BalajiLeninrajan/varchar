# varchar

`varchar` is a deliberately absurd database: its entire authoritative state—schemas, constraints, sequence state, and rows—is one UTF-8 `String`, and every supported `SELECT` scans that string with generated regular expressions.

It is a real parser, type checker, storage codec, and query engine wrapped around a joke premise. It is also a toy. Do not use it for production data, durability, concurrent writers, or anything whose loss would make your day worse.

## How it is put together

The Cargo workspace has two parts:

- `varchar` is the platform-neutral core. It owns the database string and implements storage validation, SQL parsing, type checking, regex planning, and query execution.
- `varchar-cli` builds the native `varchar` binary. It owns files, atomic replacement, terminal input, and output formatting.

The core has no filesystem or terminal API. Parsed schemas, syntax trees, compiled regexes, and result rows may exist temporarily, but the one string remains the only authoritative database state.

Every supported `SELECT` compiles the scans for all participating tables into one regex—an alternation for joins. Safe predicate leaves from a top-level conjunction become exact regex prefilters; Rust evaluates the remaining Boolean expression against decoded values. For a join, source-local residuals run before rows are retained, `ON` conditions run during left-to-right nested loops, and cross-source residuals run afterward. `EXPLAIN REGEX` exposes the generated scan prefilter, which may represent only part of the `WHERE` expression, so the trick stays visible. `SelectExplanation::pattern_is_exact` reports which case a caller has: `true` means the pattern expresses all row filtering and selects exactly the rows the query retains, `false` means the pattern is a prefilter that over-selects and Rust-side evaluation decides the rest. A join is never exact, because its pattern is an alternation over whole source rows and `ON` conditions run in Rust. Clauses that never eliminate source rows—projection, and any ordering or pagination the dialect supports—are not represented by the pattern either, and they do not make the flag `false`.

## Quick start

The repository pins its Rust version and required components in `rust-toolchain.toml`.

```console
cargo build --workspace

cargo run -p varchar-cli -- init ./demo.varchar
cargo run -p varchar-cli -- exec ./demo.varchar \
  "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, active BOOLEAN)"
cargo run -p varchar-cli -- exec ./demo.varchar \
  "INSERT INTO users (name, active) VALUES ('Ada', TRUE)"
cargo run -p varchar-cli -- exec ./demo.varchar \
  "SELECT name, active FROM users WHERE id = 1 AND name LIKE 'A%'"
cargo run -p varchar-cli -- exec ./demo.varchar \
  "EXPLAIN REGEX SELECT name FROM users WHERE active = TRUE"
cargo run -p varchar-cli -- dump ./demo.varchar
```

To use the REPL:

```console
cargo run -p varchar-cli -- shell ./demo.varchar
```

SQL statements in the shell end with `;`. Use `.dump` to inspect the raw database string and `.quit` to leave.

`init` refuses to overwrite an existing file. Successful mutations replace the file through a temporary file in the same directory; failed SQL or persistence leaves the previous database untouched. This provides atomic visibility for ordinary local use, not crash-proof durability or coordination between concurrent writers.

## Supported SQL

Varchar accepts one statement at a time, with an optional trailing semicolon.

| Operation | Supported shape |
| --- | --- |
| Create | `CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT DEFAULT 'anonymous', active BOOLEAN, CHECK (name != ''))` |
| Insert | `INSERT INTO users VALUES (1, 'Ada', TRUE)` |
| Insert by column | `INSERT INTO users (name) VALUES ('Grace')` |
| Select | `SELECT * FROM users` or a named projection, optionally followed by `ORDER BY`, `LIMIT`, and `OFFSET` |
| Join | `SELECT users.name, posts.body FROM users JOIN posts ON users.id = posts.user_id` |
| Update | `UPDATE users SET active = FALSE WHERE id = 1` |
| Delete | `DELETE FROM users WHERE name LIKE 'A%'` |
| Explain | `EXPLAIN REGEX SELECT name FROM users WHERE active = TRUE` |

Column types are `TEXT`, signed 64-bit `INTEGER`, and `BOOLEAN`. Columns are nullable unless declared `NOT NULL`; `NULL` is represented as its own typed value. A column may declare one literal `DEFAULT`, including an explicit `DEFAULT NULL`.

Varchar supports one single-column primary key per table, any number of single-column UNIQUE constraints, and single-column foreign keys. Constraints may be written inline:

```sql
CREATE TABLE users (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  email TEXT UNIQUE
);

CREATE TABLE posts (
  id INTEGER PRIMARY KEY,
  user_id INTEGER REFERENCES users(id),
  body TEXT NOT NULL
);
```

The equivalent table-level forms include `PRIMARY KEY (id)`, `UNIQUE (email)`, and `FOREIGN KEY (user_id) REFERENCES users(id)`. Composite key and UNIQUE constraints are not supported. A primary key implies `NOT NULL` and is unique across the table; one UNIQUE declaration on that same column is accepted and normalized away. A non-primary UNIQUE column rejects duplicate non-NULL values but permits multiple NULLs. Text equality remains case- and normalization-sensitive. A foreign key must reference an existing primary-key column with the same type; UNIQUE columns are not foreign-key targets. Foreign-key columns remain nullable unless they also use `NOT NULL`; a `NULL` value does not need a matching parent row.

Key and CHECK constraints are enforced when data is inserted or updated and when a persisted database is loaded. Candidate validation checks primary keys and sequence state first, then UNIQUE, CHECK, and foreign keys. Parent-key changes and parent-row deletions use `RESTRICT`: they fail while a child row contains that key. Like every failed mutation, a constraint violation leaves the authoritative string unchanged.

An `INTEGER PRIMARY KEY` can use `AUTOINCREMENT` or `AUTO_INCREMENT` in its inline column definition:

```sql
CREATE TABLE messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  body TEXT NOT NULL
);

INSERT INTO messages (body) VALUES ('first');
INSERT INTO messages VALUES (NULL, 'second');
```

Omitting the generated column from a named-column insert, or explicitly inserting `NULL`, generates the next positive integer. A new table persists a high-water mark of `0`, so its first generated key is `1`. The mark advances for larger explicit inserts and updates, never falls after deletion, and survives reloads. Zero and negative explicit values do not advance it. Overflow and every other failed mutation leave both rows and the high-water mark unchanged.

Defaults are literal values and apply only when a column is omitted from a named-column insert:

```sql
CREATE TABLE jobs (
  id INTEGER PRIMARY KEY DEFAULT 1,
  state TEXT NOT NULL DEFAULT 'queued',
  note TEXT DEFAULT NULL
);

INSERT INTO jobs (id) VALUES (2);             -- state is 'queued'
INSERT INTO jobs (id, state) VALUES (3, NULL); -- rejected: explicit NULL is not defaulted
```

Explicit `NULL` never invokes a default, and positional inserts still require exactly one value per column. Defaults are applied before auto-increment generation and final type/nullability validation. `DEFAULT NULL` is invalid for a final `NOT NULL` or primary-key column, and an auto-increment column cannot have any default. A correctly typed non-NULL default is allowed on an ordinary primary key. Existing rows are never backfilled.

CHECK constraints may be inline or table-level. An inline CHECK is still table-local and may reference any unqualified column, including a column declared later:

```sql
CREATE TABLE ranges (
  start INTEGER CHECK (finish IS NULL OR finish >= 0),
  finish INTEGER
);

CREATE TABLE tasks (
  state TEXT DEFAULT 'queued',
  attempts INTEGER CHECK (attempts >= 0 OR attempts IS NULL)
);
```

CHECK expressions support parentheses, `AND`, `OR`, `=`, `!=`, `<`, `<=`, `>`, `>=`, `LIKE`, `IS NULL`, `IS NOT NULL`, and nonempty literal-list `IN (...)`. Ordered operands and non-NULL `IN` members must have the column's type; INTEGER uses signed numeric order, BOOLEAN uses `FALSE < TRUE`, and TEXT uses case- and normalization-sensitive lexicographic order. `IN` follows SQL NULL semantics, so `value IN (NULL)` is unknown for every value rather than false. CHECK rejects only false and accepts both true and unknown. Direct comparisons to `NULL` remain type errors.

CHECK declarations are preserved in source order, including duplicates, and the first failing declaration is reported. Defaults and generated auto-increment keys are applied before CHECK evaluation. Inserts and updates validate the complete candidate and remain byte-for-byte atomic on failure; persisted rows are checked again during reload. `max_predicates` applies cumulatively to every CHECK on one table, counting one unit per ordinary predicate and one unit per `IN` member. Qualified references, unknown columns, and cross-table references are rejected.

`WHERE` supports parentheses plus `AND`/`OR`, with `AND` binding more tightly than `OR`. Predicate leaves are:

- `=`, `!=`, `<`, `<=`, `>`, `>=`
- `IN (literal, ...)`, with at least one literal
- `LIKE`, where `%` matches any sequence and `_` matches one Unicode scalar
- `IS NULL`, `IS NOT NULL`

Backslash escapes `%`, `_`, and backslash inside a `LIKE` pattern. Comparisons, `IN`, and `LIKE` use SQL three-valued truth for nullable columns: a `NULL` left value produces unknown, and `WHERE` retains only true. Direct comparison to a `NULL` literal is a type error for every comparison operator; use `IS NULL` or `IS NOT NULL`. Ordered operands must have the column's type: integers use signed numeric order, booleans use `FALSE < TRUE`, and text uses case-sensitive decoded Unicode-scalar order without normalization. Every non-`NULL` `IN` member must have the column's type; an all-`NULL` list is valid for every column type, and duplicate members are preserved without changing the result. For a non-`NULL` left value, `IN` returns true for any equal non-`NULL` member, otherwise unknown when the list contains `NULL`, and otherwise false; a `NULL` left value is always unknown. Every list member is resolved left to right before execution, even when an earlier member matches. All leaves are resolved and type-checked before execution, even when runtime short-circuiting would skip one. Keywords and unquoted ASCII identifiers are case-insensitive. Text values and `LIKE` matching are case-sensitive.

`SELECT` supports inner equijoins using either `JOIN` or `INNER JOIN`:

```sql
SELECT users.name, posts.body
FROM users
INNER JOIN posts ON users.id = posts.user_id
WHERE posts.body LIKE 'A%';
```

An `ON` clause contains column-to-column equality terms joined by `AND`. Additional join clauses form a left-to-right chain, and later clauses may refer to any earlier source. Column references in projections, `ON`, and `WHERE` may be table-qualified; a bare column is accepted only when exactly one participating table contains that name. `table.*` expands one table in schema order, while unqualified `*` expands all sources in `FROM`/`JOIN` order.

Join equality uses SQL null semantics: `NULL` never equals any value, including another `NULL`. Duplicate and many-to-many matches are preserved. Without ordering, results use deterministic nested-loop order: physical row order from the `FROM` table, followed by physical row order from each joined table left to right.

`ORDER BY` accepts one or more real source columns, each optionally followed by `ASC` or `DESC`. Sort columns need not be projected, duplicate terms are retained, and joined columns may be qualified only with their real table name. Unqualified names must be unambiguous. Ordering is lexicographic across INTEGER, BOOLEAN (`FALSE < TRUE`), and decoded TEXT Unicode scalars. Ascending order puts NULL after non-NULL values; descending order puts NULL first. Final ties preserve the same physical or nested-loop order an unordered query would have produced.

```sql
SELECT children.name
FROM parents JOIN children ON parents.id = children.parent_id
ORDER BY parents.created_at, children.name DESC;
```

Aliases, ordinals, expressions, `COLLATE`, and configurable NULL placement are not supported as sort terms.

A `SELECT` tail has the fixed shape `[ORDER BY ...] [LIMIT unsigned_integer] [OFFSET unsigned_integer]`. `OFFSET` is valid without `LIMIT`; both accept zero, leading zeroes, and values through `18446744073709551615`. Filtering and joins happen first, then ordering, then offset skipping, then limiting. An offset beyond the result cardinality returns the normal column metadata with no rows. Without `ORDER BY`, skipped rows are not cloned into the result and execution stops as soon as the limit is filled. An ordered query with a `LIMIT` retains only the `OFFSET + LIMIT` rows that could still land in the window and drops the rest as it scans; without a `LIMIT` it retains every qualifying row. `LIMIT 0` still parses, resolves, validates, generates, and compiles the query plan and charges output-column metadata, but it performs no row scan, join comparisons, ordered-row retention, or sorting.

```sql
SELECT * FROM events LIMIT 25;
SELECT * FROM events OFFSET 100;
SELECT * FROM events ORDER BY created_at DESC LIMIT 25 OFFSET 50;
```

Each library result column includes its display label and the table/column it originated from. When a joined result contains the same label from different sources, the CLI qualifies those headers with their table names.

Unconstrained tables retain duplicate rows. Projection order, duplicate projected columns, and physical insertion order are preserved.

The intentionally small dialect does not include outer joins, aliases, self-joins, aggregation, subqueries, unary `NOT`, quoted identifiers, comments, statement batches, or schema alteration. Unsupported syntax is rejected rather than partially interpreted.

## The one string

The storage format is deterministic, versioned, printable, and one line long. A representative database looks like this:

```text
V2;~S|users|id:I:!|name:T:?|active:B:?;~P|users|id;~A|users|id|I1;~R|users|I1|TAda|B1;
```

Schema and row records carry explicit tags. Key constraints are metadata records before the row records: `~P|users|id;` declares a primary key, while `~F|posts|user_id|users|id;` declares a foreign key. An auto-incrementing key has exactly one record such as `~A|users|id|I42;`, placed after that table's primary- and foreign-key metadata. Its nonnegative high-water mark must cover every stored key for the generated column.

V2 remains the canonical format for databases that use only legacy metadata. A first nonredundant V3 feature—DEFAULT, UNIQUE, or CHECK—atomically changes the header to `V3;`. DEFAULT records such as `~D|jobs|state|Tqueued;` use canonical typed cells, with explicit `DEFAULT NULL` encoded as `N`; UNIQUE records use `~U|users|email;`. Each CHECK is a `~C` record containing a resolved, column-index-based flat preorder program, for example `~C|tasks|GE|1|I0;`. Logical nodes store child counts; LIKE stores wildcard/literal atoms; IN stores canonical typed cells. Per table, DEFAULT records follow optional auto-increment metadata in increasing column order, followed by UNIQUE records in increasing column order and CHECK records in declaration order. Loading accepts V2 and V3 without rewriting either one, V3 never downgrades during later mutations, and a V3-only record under a V2 header is corruption. Redundant primary-key UNIQUE declarations emit no `~U` record and do not require V3. V1 blobs remain unsupported rather than being migrated implicitly.

Cell prefixes distinguish text, integers, booleans, and nulls, while structural and line-breaking characters are escaped reversibly. Loading validates the complete header, schemas, constraint metadata, key integrity, escapes, row widths, types, and canonical encoding; malformed records are never silently skipped.

The format is inspectable for fun and debugging, but callers should treat it as an encoded value rather than edit it by hand. Use `varchar dump` or `Database::as_str()` to see it.

## Library use

The core API keeps persistence in the host application:

```rust
use varchar::{Database, Outcome};

fn main() -> Result<(), varchar::Error> {
    let mut db = Database::new();
    db.execute("CREATE TABLE messages (body TEXT NOT NULL)")?;
    db.execute("INSERT INTO messages VALUES ('hello')")?;

    let explanation = db.explain_select("SELECT body FROM messages WHERE body LIKE 'h%'")?;
    println!("{}", explanation.pattern());
    assert_eq!(explanation.sources(), &["messages"]);
    // A pushed `LIKE` leaves no residual, so the pattern expresses the whole
    // `WHERE` clause.
    assert!(explanation.pattern_is_exact());

    if let Outcome::Rows(rows) = db.execute("SELECT body FROM messages")? {
        assert_eq!(rows.rows().len(), 1);
    }

    let persisted: String = db.into_string();
    assert!(persisted.starts_with("V2;"));
    Ok(())
}
```

Use `Database::from_string` to validate and reopen a persisted blob. Errors distinguish malformed SQL, unsupported features, schema, type, and constraint failures, corrupt storage, regex failures, resource-limit exhaustion, recoverable failures from explicit allocation reservations, and internal capacity exhaustion. A failed mutation leaves the original string byte-for-byte unchanged.

The diagnostic API is structured and deliberately smaller than the engine's internal error representation:

```rust
use varchar::{Database, Error, Limits, Resource};

let limits = Limits {
    max_sql_bytes: 8,
    ..Limits::default()
};
let mut db = Database::with_limits(limits);
let before = db.as_str().to_owned();
let error = db
    .execute("CREATE TABLE messages (body TEXT)")
    .expect_err("the statement exceeds the configured SQL limit");

assert!(matches!(
    error,
    Error::ResourceLimit {
        resource: Resource::SqlBytes,
        limit: 8,
    }
));
assert_eq!(db.as_str(), before);
```

Match on `Error` variants for structured diagnostics; human-readable `Display` text is intended for people and may change. Parse and unsupported-syntax errors carry half-open UTF-8 byte offsets into the original SQL input. Corrupt-storage offsets refer to bytes in the encoded database blob, not decoded values. A configured limit failure includes both its typed `Resource` and limit. It returns no partial result, and a failed mutation—including one rejected by a limit—leaves the authoritative blob byte-for-byte unchanged.

Query rows, projected-column metadata, provenance, and `SelectExplanation` values are immutable snapshots produced by the engine. Inspect them through their accessors; a `RowSet` can also be consumed with `into_rows` or `into_parts` when the caller needs owned values.

## WebAssembly

The core is kept compatible with both `wasm32-unknown-unknown` and `wasm32-wasip1`. It avoids native libraries, ambient filesystem access, networking, randomness, and threads, and applies configured limits to inputs, generated patterns, logical `SELECT` working/output charges, storage reconstruction, join execution, and regex backtracking.

There is no public JavaScript/WASM package in v1. A future browser adapter can pass the complete blob into the same core, execute one statement per call, and persist the returned blob in a browser-owned store. A future WASI adapter can provide capability-based persistence separately.

## Performance and limits

The punchline is also the performance model:

- Except for `LIMIT 0`, every executed query scans the database string once. Unordered single-table queries are **O(n)** in database size; joins then use budgeted, materialized nested loops whose work can grow to the product of participating row counts. Unordered results stream in qualifying order, apply `OFFSET` before cloning output values, and stop once `LIMIT` is full. For `r` qualifying rows and a window of `w = OFFSET + LIMIT` rows, `ORDER BY` retains `min(r, w)` projections, each plus one owned value per sort key and a tie-breaking ordinal, in a max-heap keyed by the sort order; a row that cannot beat the heap root is dropped without being cloned, and one that can evicts the root in **O(log w)**. Without a `LIMIT` the window is open-ended and every qualifying row is retained. The retained rows are then sorted with **O(min(r, w) log min(r, w))** row comparisons; each comparison may inspect multiple keys, and each TEXT-key comparison may scan a shared Unicode-scalar prefix. The sort is allocation-free and unstable internally, with the ordinal preserving input order on final ties; only then is the pagination window applied. `LIMIT 0` completes planning but skips the scan and join traversal.
- Every mutation builds and validates a candidate string before replacing the old state. Inserts and schema changes copy the authoritative blob, while updates and deletes scan and finish a candidate even when no row matches, so all mutation paths are **O(n)** in database size. A zero-match update or delete installs a separately validated but byte-identical state.
- There are no data indexes, transactions, WALs, or concurrent-writer guarantees.
- Inputs, generated regexes, join execution work, regex backtracking, and auxiliary storage validation state are bounded. The private storage-working bound is four times `max_database_bytes`; it conservatively charges catalog reconstruction, owned metadata, and validation indexes separately from the authoritative string's `DatabaseBytes` limit. `max_predicates` bounds each `WHERE` independently and, separately, the cumulative CHECK predicate units for each table: ordinary predicate leaves consume one unit, `IN` consumes one unit per list member, and `AND`, `OR`, and parentheses consume none. `SELECT` working state and returned output have independent 32 MiB logical-byte defaults: the working budget conservatively charges transient decoded rows, one reusable residual-evaluation stack, rows plus pointer state retained for joins, and every ordered pending-row descriptor, projected value, owned sort key, text payload, and `u64` ordinal before allocation. An ordered row evicted from a bounded pagination window refunds its charge, so the ordered charge tracks live retained rows rather than every row scanned. Ordered sorting itself uses no scratch allocation. `max_query_output_bytes` independently bounds projection-location preflight; a fresh output budget then charges returned `RowSet` metadata and only the final rows remaining after pagination, or materialized `SelectExplanation` patterns, sources, and column metadata. These are safety rails, not a total query or process memory cap; they exclude other planning allocations, regex-engine scratch space, the authoritative string, allocator overhead and capacity beyond conservative descriptor charges, and some mutation-candidate allocation. Logical charges include target-layout sizes, so exact boundaries can differ between 32-bit and 64-bit builds. `UPDATE` and `DELETE` do not consume the `SELECT` working budget. Both `SELECT` budgets can be live at once, and a limit failure returns no partial result or mutation.

Varchar is meant to be understandable, inspectable, and funny—not fast.

## Development

```console
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo check -p varchar --target wasm32-unknown-unknown
cargo check -p varchar --target wasm32-wasip1
wasm-pack test --node crates/varchar
```

Unit tests live in dedicated child-module `tests.rs` files; implementation
modules contain only the `#[cfg(test)] mod tests;` declaration. Public and
cross-module behavior remains covered by the integration suites under each
crate's top-level `tests/` directory.

CI runs the same native and WebAssembly gates.
