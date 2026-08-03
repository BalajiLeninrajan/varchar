# varchar

`varchar` is a deliberately absurd database: its entire authoritative state—schemas, constraints, sequence state, and rows—is one UTF-8 `String`, and every supported `SELECT` scans that string with generated regular expressions.

It is a real parser, type checker, storage codec, and query engine wrapped around a joke premise. It is also a toy. Do not use it for production data, durability, concurrent writers, or anything whose loss would make your day worse.

## How it is put together

The Cargo workspace has two parts:

- `varchar` is the platform-neutral core. It owns the database string and implements storage validation, SQL parsing, type checking, regex planning, and query execution.
- `varchar-cli` builds the native `varchar` binary. It owns files, atomic replacement, terminal input, and output formatting.

The core has no filesystem or terminal API. Parsed schemas, syntax trees, compiled regexes, and result rows may exist temporarily, but the one string remains the only authoritative database state.

Every supported `SELECT` compiles the scans for all participating tables into one regex—an alternation for joins. Safe predicate leaves from a top-level conjunction become exact regex prefilters; Rust evaluates the remaining Boolean expression against decoded values. For a join, source-local residuals run before rows are retained, `ON` conditions run during left-to-right nested loops, and cross-source residuals run afterward. `EXPLAIN REGEX` exposes the generated scan prefilter, which may represent only part of the `WHERE` expression, so the trick stays visible.

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
| Create | `CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, active BOOLEAN)` |
| Insert | `INSERT INTO users VALUES (1, 'Ada', TRUE)` |
| Insert by column | `INSERT INTO users (name) VALUES ('Grace')` |
| Select | `SELECT * FROM users` or a named, ordered projection |
| Join | `SELECT users.name, posts.body FROM users JOIN posts ON users.id = posts.user_id` |
| Update | `UPDATE users SET active = FALSE WHERE id = 1` |
| Delete | `DELETE FROM users WHERE name LIKE 'A%'` |
| Explain | `EXPLAIN REGEX SELECT name FROM users WHERE active = TRUE` |

Column types are `TEXT`, signed 64-bit `INTEGER`, and `BOOLEAN`. Columns are nullable unless declared `NOT NULL`; `NULL` is represented as its own typed value.

Varchar supports one single-column primary key per table and single-column foreign keys. Constraints may be written inline:

```sql
CREATE TABLE users (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL
);

CREATE TABLE posts (
  id INTEGER PRIMARY KEY,
  user_id INTEGER REFERENCES users(id),
  body TEXT NOT NULL
);
```

The equivalent table-level forms are `PRIMARY KEY (id)` and `FOREIGN KEY (user_id) REFERENCES users(id)`. Composite keys are not supported. A primary key implies `NOT NULL` and is unique across the table. A foreign key must reference an existing primary-key column with the same type. Foreign-key columns remain nullable unless they also use `NOT NULL`; a `NULL` value does not need a matching parent row.

Key constraints are checked when data is inserted or updated and when a persisted database is loaded. Parent-key changes and parent-row deletions use `RESTRICT`: they fail while a child row contains that key. Like every failed mutation, a key violation leaves the authoritative string unchanged.

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

`WHERE` supports parentheses plus `AND`/`OR`, with `AND` binding more tightly than `OR`. Predicate leaves are:

- `=`, `!=`
- `LIKE`, where `%` matches any sequence and `_` matches one Unicode scalar
- `IS NULL`, `IS NOT NULL`

Backslash escapes `%`, `_`, and backslash inside a `LIKE` pattern. Comparisons and `LIKE` use SQL three-valued truth for nullable columns: a `NULL` input produces unknown, and `WHERE` retains only true. Direct `= NULL` and `!= NULL` comparisons are type errors; use `IS NULL` or `IS NOT NULL`. All leaves are resolved and type-checked before execution, even when runtime short-circuiting would skip one. Keywords and unquoted ASCII identifiers are case-insensitive. Text values and `LIKE` matching are case-sensitive.

`SELECT` supports inner equijoins using either `JOIN` or `INNER JOIN`:

```sql
SELECT users.name, posts.body
FROM users
INNER JOIN posts ON users.id = posts.user_id
WHERE posts.body LIKE 'A%';
```

An `ON` clause contains column-to-column equality terms joined by `AND`. Additional join clauses form a left-to-right chain, and later clauses may refer to any earlier source. Column references in projections, `ON`, and `WHERE` may be table-qualified; a bare column is accepted only when exactly one participating table contains that name. `table.*` expands one table in schema order, while unqualified `*` expands all sources in `FROM`/`JOIN` order.

Join equality uses SQL null semantics: `NULL` never equals any value, including another `NULL`. Duplicate and many-to-many matches are preserved. Results use deterministic nested-loop order: physical row order from the `FROM` table, followed by physical row order from each joined table left to right.

Each library result column includes its display label and the table/column it originated from. When a joined result contains the same label from different sources, the CLI qualifies those headers with their table names.

Unconstrained tables retain duplicate rows. Projection order, duplicate projected columns, and physical insertion order are preserved.

The intentionally small dialect does not include outer joins, aliases, self-joins, aggregation, ordering, subqueries, unary `NOT`, quoted identifiers, comments, statement batches, or schema alteration. Unsupported syntax is rejected rather than partially interpreted.

## The one string

The storage format is deterministic, versioned, printable, and one line long. A representative database looks like this:

```text
V2;~S|users|id:I:!|name:T:?|active:B:?;~P|users|id;~A|users|id|I1;~R|users|I1|TAda|B1;
```

Schema and row records carry explicit tags. Key constraints are metadata records before the row records: `~P|users|id;` declares a primary key, while `~F|posts|user_id|users|id;` declares a foreign key. An auto-incrementing key has exactly one record such as `~A|users|id|I42;`, placed after that table's primary- and foreign-key metadata. Its nonnegative high-water mark must cover every stored key for the generated column. V2 is a strict format bump: V1 blobs are rejected rather than migrated implicitly.

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

The core is kept compatible with both `wasm32-unknown-unknown` and `wasm32-wasip1`. It avoids native libraries, ambient filesystem access, networking, randomness, and threads, and applies configured limits to inputs, generated patterns, logical `SELECT` working/output charges, join execution, and regex backtracking.

There is no public JavaScript/WASM package in v1. A future browser adapter can pass the complete blob into the same core, execute one statement per call, and persist the returned blob in a browser-owned store. A future WASI adapter can provide capability-based persistence separately.

## Performance and limits

The punchline is also the performance model:

- Every query scans the database string once. Single-table queries are **O(n)** in database size; joins then use budgeted, materialized nested loops whose work can grow to the product of participating row counts.
- Every mutation builds and validates a candidate string before replacing the old state. Inserts and schema changes copy the authoritative blob, while updates and deletes scan and finish a candidate even when no row matches, so all mutation paths are **O(n)** in database size. A zero-match update or delete installs a separately validated but byte-identical state.
- There are no data indexes, transactions, WALs, or concurrent-writer guarantees.
- Inputs, generated regexes, join execution work, and regex backtracking are bounded. `max_predicates` bounds each `WHERE` independently by predicate units: the current operators consume one unit per leaf, while `AND`, `OR`, and parentheses consume none. `SELECT` working state and returned output have independent 32 MiB logical-byte defaults: the working budget conservatively charges transient decoded rows, one reusable residual-evaluation stack, and rows plus pointer state retained for joins. `max_query_output_bytes` independently bounds projection-location preflight; a fresh output budget then charges returned `RowSet` metadata and projected rows or materialized `SelectExplanation` patterns, sources, and column metadata. These are safety rails, not a total query or process memory cap; they exclude other planning allocations, regex-engine scratch space, catalog and integrity-index allocations, the authoritative string, allocator overhead and capacity beyond the conservative descriptor charges, and mutation candidates. Logical charges include target-layout sizes, so exact boundaries can differ between 32-bit and 64-bit builds. `UPDATE` and `DELETE` do not consume the `SELECT` working budget. Both `SELECT` budgets can be live at once, and a limit failure returns no partial result or mutation.

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
