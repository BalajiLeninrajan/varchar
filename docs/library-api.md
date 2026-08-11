# Library API

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

Tabular rows, result-column metadata, provenance, and `SelectExplanation` values are immutable snapshots produced by the engine. `SELECT`, `SHOW TABLES`, `DESCRIBE`, and `SHOW CREATE TABLE` all return tabular data as `Outcome::Rows`. Inspect snapshots through their accessors; a `RowSet` can also be consumed with `into_rows` or `into_parts` when the caller needs owned values.

