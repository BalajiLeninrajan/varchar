# varchar

`varchar` is a deliberately absurd database: its entire authoritative state—schemas, constraints, sequence state, and rows—is one UTF-8 `String`, and every supported `SELECT` scans that string with regexes.

It is a real parser, type checker, storage codec, and query engine. It is also a toy. Do not use it for production data, durability, concurrent writers, or anything whose loss would make your day worse.

```console
$ varchar exec ./demo.varchar "SELECT name FROM users WHERE active = TRUE"
$ varchar dump ./demo.varchar
V2;~S|users|id:I:!|name:T:?|active:B:?;~P|users|id;~A|users|id|I1;~R|users|I1|TAda|B1;
```

## Quick start

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

See the [SQL reference](docs/sql-reference.md) for the exact semantics of every clause.

## Library use

The core crate is platform-neutral and keeps persistence in the host application:

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

See the [library API guide](docs/library-api.md) for errors, limits, and `EXPLAIN REGEX` output.

## Caveats

Varchar is meant to be understandable, inspectable, and funny—not fast. Every query scans the whole database string, every mutation rewrites it, and there are no indexes, transactions, WALs, or concurrent-writer guarantees. Failed statements leave the stored string byte-for-byte unchanged.

## Documentation

- [SQL reference](docs/sql-reference.md) — the complete dialect, clause by clause
- [Library API](docs/library-api.md) — embedding the core crate, errors, and limits
- [Architecture](docs/architecture.md) — how the workspace, regex planner, and mutation planner fit together
- [Storage format](docs/storage-format.md) — the one string, record by record
- [Performance and limits](docs/performance.md) — complexity, resource budgets, and what is bounded
- [WebAssembly](docs/wasm.md) — `wasm32-unknown-unknown` and `wasm32-wasip1` support
- [Development](docs/development.md) — build, lint, and test gates
