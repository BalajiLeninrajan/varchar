# varchar

[![CI](https://github.com/BalajiLeninrajan/varchar/actions/workflows/ci.yml/badge.svg)](https://github.com/BalajiLeninrajan/varchar/actions/workflows/ci.yml)
[![varchar on crates.io](https://img.shields.io/crates/v/varchar.svg?label=varchar)](https://crates.io/crates/varchar)
[![varchar-cli on crates.io](https://img.shields.io/crates/v/varchar-cli.svg?label=varchar-cli)](https://crates.io/crates/varchar-cli)
[![docs.rs](https://img.shields.io/docsrs/varchar?label=docs.rs)](https://docs.rs/varchar)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/BalajiLeninrajan/varchar/blob/main/LICENSE)

`varchar` is a really dumb SQL DB: its entire authoritative state—schemas, constraints, sequence state, and rows—is one UTF-8 `String`, and every supported `SELECT` scans that string with regexes.

It is a real parser, type checker, storage codec, and query engine. It is also a toy. Do not use it for production data, durability, concurrent writers, or anything whose loss would make your day worse.

```console
$ varchar exec ./demo.varchar "SELECT name FROM users WHERE active = TRUE"
$ varchar dump ./demo.varchar
V2;~S|users|id:I:!|name:T:?|active:B:?;~P|users|id;~A|users|id|I1;~R|users|I1|TAda|B1;
```

## Install

The workspace publishes two crates. [`varchar`](https://crates.io/crates/varchar) is the
embeddable engine; [`varchar-cli`](https://crates.io/crates/varchar-cli) is the native
command-line front end and installs a `varchar` binary.

```console
cargo install varchar-cli
```

```console
cargo add varchar
```

Or build from a clone of the repository:

```console
git clone https://github.com/BalajiLeninrajan/varchar
cargo build --workspace
```

## Quick start

```console
varchar init ./demo.varchar
varchar exec ./demo.varchar \
  "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, active BOOLEAN)"
varchar exec ./demo.varchar \
  "INSERT INTO users (name, active) VALUES ('Ada', TRUE)"
varchar exec ./demo.varchar \
  "SELECT name, active FROM users WHERE id = 1 AND name LIKE 'A%'"
varchar exec ./demo.varchar \
  "EXPLAIN REGEX SELECT name FROM users WHERE active = TRUE"
varchar dump ./demo.varchar
```

To use the REPL:

```console
varchar shell ./demo.varchar
```

From a clone, replace `varchar` with `cargo run -p varchar-cli --` in any of the commands above.

SQL statements in the shell end with `;`. Use `.dump` to inspect the raw database string and `.quit` to leave.

`init` refuses to overwrite an existing file. Successful mutations replace the file through a temporary file in the same directory; failed SQL or persistence leaves the previous database untouched. This provides atomic visibility for ordinary local use, not crash-proof durability or coordination between concurrent writers.

## What it can do

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
| Show tables | `SHOW TABLES` |
| Describe table | `DESCRIBE users` |
| Show create table | `SHOW CREATE TABLE users` |
| Explain | `EXPLAIN REGEX SELECT name FROM users WHERE active = TRUE` |

Columns are `TEXT`, signed 64-bit `INTEGER`, or `BOOLEAN`, with `NOT NULL`, literal `DEFAULT`, single-column `PRIMARY KEY`, `AUTOINCREMENT`, `UNIQUE`, `CHECK`, and single-column foreign keys with `RESTRICT`, `CASCADE`, and `SET NULL` actions.

The intentionally small dialect does not include outer joins, aliases, self-joins, aggregation, subqueries, unary `NOT`, comments, statement batches, or schema alteration. Unsupported syntax is rejected rather than partially interpreted.

See the [SQL reference](https://github.com/BalajiLeninrajan/varchar/blob/main/docs/sql-reference.md) for the exact semantics of every clause.

## Library use

The core [`varchar`](https://crates.io/crates/varchar) crate is platform-neutral and keeps
persistence in the host application:

```rust
use varchar::{Database, Outcome};

let mut db = Database::new();
db.execute("CREATE TABLE messages (body TEXT NOT NULL)")?;
db.execute("INSERT INTO messages VALUES ('hello')")?;

if let Outcome::Rows(rows) = db.execute("SELECT body FROM messages")? {
    assert_eq!(rows.rows().len(), 1);
}

let persisted: String = db.into_string();
# Ok::<(), varchar::Error>(())
```

See the [library API guide](https://github.com/BalajiLeninrajan/varchar/blob/main/docs/library-api.md) for errors, limits, and `EXPLAIN REGEX` output, and [docs.rs/varchar](https://docs.rs/varchar) for the generated API documentation.

## Caveats

Varchar is meant to be understandable, inspectable, and funny—not fast. Every query scans the whole database string, every mutation rewrites it, and there are no indexes, transactions, WALs, or concurrent-writer guarantees. Failed statements leave the stored string byte-for-byte unchanged.

## Documentation

- [docs.rs/varchar](https://docs.rs/varchar) — generated API documentation for the core crate
- [SQL reference](https://github.com/BalajiLeninrajan/varchar/blob/main/docs/sql-reference.md) — the complete dialect, clause by clause
- [Library API](https://github.com/BalajiLeninrajan/varchar/blob/main/docs/library-api.md) — embedding the core crate, errors, and limits
- [Architecture](https://github.com/BalajiLeninrajan/varchar/blob/main/docs/architecture.md) — how the workspace, regex planner, and mutation planner fit together
- [Storage format](https://github.com/BalajiLeninrajan/varchar/blob/main/docs/storage-format.md) — the one string, record by record
- [Performance and limits](https://github.com/BalajiLeninrajan/varchar/blob/main/docs/performance.md) — complexity, resource budgets, and what is bounded
- [WebAssembly](https://github.com/BalajiLeninrajan/varchar/blob/main/docs/wasm.md) — `wasm32-unknown-unknown` and `wasm32-wasip1` support
- [Development](https://github.com/BalajiLeninrajan/varchar/blob/main/docs/development.md) — build, lint, and test gates
