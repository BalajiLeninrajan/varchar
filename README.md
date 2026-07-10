# varchar

`varchar` is a deliberately absurd database: its entire authoritative state—schemas and rows—is one UTF-8 `String`, and every supported `SELECT` filters that string with one generated regular expression.

It is a real parser, type checker, storage codec, and query engine wrapped around a joke premise. It is also a toy. Do not use it for production data, durability, concurrent writers, or anything whose loss would make your day worse.

## How it is put together

The Cargo workspace has two parts:

- `varchar` is the platform-neutral core. It owns the database string and implements storage validation, SQL parsing, type checking, regex planning, and query execution.
- `varchar-cli` builds the native `varchar` binary. It owns files, atomic replacement, terminal input, and output formatting.

The core has no filesystem or terminal API. Parsed schemas, syntax trees, compiled regexes, and result rows may exist temporarily, but the one string remains the only authoritative database state.

Every supported `SELECT` compiles all of its predicates into exactly one regex. Rust decodes and projects rows after the regex matches them, but it does not perform a second predicate-filtering pass. `EXPLAIN REGEX` exposes the generated pattern so the trick stays visible.

## Quick start

The repository pins its Rust version and required components in `rust-toolchain.toml`.

```console
cargo build --workspace

cargo run -p varchar-cli -- init ./demo.varchar
cargo run -p varchar-cli -- exec ./demo.varchar \
  "CREATE TABLE users (id INTEGER NOT NULL, name TEXT, active BOOLEAN)"
cargo run -p varchar-cli -- exec ./demo.varchar \
  "INSERT INTO users VALUES (1, 'Ada', TRUE)"
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
| Create | `CREATE TABLE users (id INTEGER NOT NULL, name TEXT, active BOOLEAN)` |
| Insert | `INSERT INTO users VALUES (1, 'Ada', TRUE)` |
| Insert by column | `INSERT INTO users (name, id) VALUES ('Grace', 2)` |
| Select | `SELECT * FROM users` or a named, ordered projection |
| Update | `UPDATE users SET active = FALSE WHERE id = 1` |
| Delete | `DELETE FROM users WHERE name LIKE 'A%'` |
| Explain | `EXPLAIN REGEX SELECT name FROM users WHERE active = TRUE` |

Column types are `TEXT`, signed 64-bit `INTEGER`, and `BOOLEAN`. Columns are nullable unless declared `NOT NULL`; `NULL` is represented as its own typed value.

`WHERE` supports terms joined with `AND`:

- `=`, `!=`
- `LIKE`, where `%` matches any sequence and `_` matches one Unicode scalar
- `IS NULL`, `IS NOT NULL`

Backslash escapes `%`, `_`, and backslash inside a `LIKE` pattern. Comparisons and `LIKE` do not match `NULL`; use `IS NULL` instead of `= NULL`. Keywords and unquoted ASCII identifiers are case-insensitive. Text values and `LIKE` matching are case-sensitive.

Duplicate rows are retained. Projection order, duplicate projected columns, and physical insertion order are preserved.

The intentionally small dialect does not include joins, aggregation, ordering, aliases, subqueries, `OR`, quoted identifiers, comments, statement batches, or schema alteration. Unsupported syntax is rejected rather than partially interpreted.

## The one string

The storage format is deterministic, versioned, printable, and one line long. A representative database looks like this:

```text
V1;~S|users|id:I:!|name:T:?|active:B:?;~R|users|I1|TAda|B1;
```

Schema and row records carry explicit tags. Cell prefixes distinguish text, integers, booleans, and nulls, while structural and line-breaking characters are escaped reversibly. Loading validates the complete header, schemas, escapes, row widths, types, and canonical encoding; malformed records are never silently skipped.

The format is inspectable for fun and debugging, but callers should treat it as an encoded value rather than edit it by hand. Use `varchar dump` or `Database::as_str()` to see it.

## Library use

The core API keeps persistence in the host application:

```rust
use varchar::{Database, Outcome};

fn main() -> Result<(), varchar::Error> {
    let mut db = Database::new();
    db.execute("CREATE TABLE messages (body TEXT NOT NULL)")?;
    db.execute("INSERT INTO messages VALUES ('hello')")?;

    let plan = db.compile_select("SELECT body FROM messages WHERE body LIKE 'h%'")?;
    println!("{}", plan.pattern());

    if let Outcome::Rows(rows) = db.execute("SELECT body FROM messages")? {
        assert_eq!(rows.rows.len(), 1);
    }

    let persisted: String = db.into_string();
    assert!(persisted.starts_with("V1;"));
    Ok(())
}
```

Use `Database::from_string` to validate and reopen a persisted blob. Errors distinguish malformed SQL, unsupported features, schema and type failures, corrupt storage, regex failures, and resource-limit exhaustion. A failed mutation leaves the original string byte-for-byte unchanged.

## WebAssembly

The core is kept compatible with both `wasm32-unknown-unknown` and `wasm32-wasip1`. It avoids native libraries, ambient filesystem access, networking, randomness, and threads, and applies bounded input, pattern, result, and regex-execution limits suitable for 32-bit WebAssembly memory.

There is no public JavaScript/WASM package in v1. A future browser adapter can pass the complete blob into the same core, execute one statement per call, and persist the returned blob in a browser-owned store. A future WASI adapter can provide capability-based persistence separately.

## Performance and limits

The punchline is also the performance model:

- Queries scan the database string, so they are **O(n)** in the size of the database.
- Inserts append a row after validation; updates and deletes rebuild the string and are **O(n)**.
- There are no indexes, caches, transactions, WALs, or concurrent-writer guarantees.
- Inputs, generated regexes, materialized results, and regex backtracking are bounded. Limit failures return no partial result or mutation.

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

CI runs the same native and WebAssembly gates.
