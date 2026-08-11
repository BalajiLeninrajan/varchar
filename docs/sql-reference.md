# SQL reference

## Statements

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

## Schema metadata

`SHOW TABLES`, `DESCRIBE table`, and `SHOW CREATE TABLE table` are read-only statements returned as `Outcome::Rows`. They inspect the validated in-memory catalog without rewriting the authoritative database string. Metadata columns use stable virtual origins under `information_schema.tables` and `information_schema.columns`.

`SHOW TABLES` returns:

| Column | Type | Nullable | Meaning |
| --- | --- | --- | --- |
| `table_name` | `TEXT` | no | Normalized table name, in catalog creation order. |

`DESCRIBE table` returns one row per column, in declaration order:

| Column | Type | Nullable | Meaning |
| --- | --- | --- | --- |
| `column_name` | `TEXT` | no | Normalized column name. |
| `data_type` | `TEXT` | no | Canonical `TEXT`, `INTEGER`, or `BOOLEAN`. |
| `nullable` | `BOOLEAN` | no | Whether the column accepts `NULL`. |
| `primary_key` | `BOOLEAN` | no | Whether the column is the table's primary key. |
| `unique` | `BOOLEAN` | no | Whether the column is semantically unique; primary keys report `true`. |
| `default_value` | `TEXT` | yes | SQL `NULL` when no default exists; otherwise the SQL literal that parses back to the default: `NULL` for an explicit `DEFAULT NULL`, a quoted TEXT literal with apostrophes doubled (`'seed'`, `'NULL'`, `'it''s'`), a decimal integer, or `TRUE`/`FALSE`. |
| `auto_increment` | `BOOLEAN` | no | Whether the column owns the table's auto-increment sequence. |

`SHOW CREATE TABLE table` returns exactly one row:

| Column | Type | Nullable | Meaning |
| --- | --- | --- | --- |
| `table_name` | `TEXT` | no | Normalized table name. |
| `create_statement` | `TEXT` | no | Canonical `CREATE TABLE` statement without a trailing semicolon. |

The generated statement preserves the catalog's schema semantics rather than the original spelling or inline-versus-table-level placement. It emits columns and CHECK declarations in catalog order, quotes reserved identifiers and literal defaults, writes foreign-key actions explicitly, and reconstructs CHECK precedence and LIKE escapes. `AUTOINCREMENT` is included, but its mutable high-water value is runtime state and is not part of `CREATE TABLE`. Referenced tables must already exist when replaying foreign-key DDL. The CLI prints the generated statement verbatim instead of applying its usual tabular TEXT escaping, so backslashes and control characters retain their SQL meaning.

```sql
SHOW TABLES;
DESCRIBE users;
SHOW CREATE TABLE users;
```

Metadata materialization is bounded by `Limits::max_query_output_bytes` and reports `Resource::QueryOutputBytes` on exhaustion. These statements do not consume the `SELECT`-specific `max_query_working_bytes` budget.

## Column types

Column types are `TEXT`, signed 64-bit `INTEGER`, and `BOOLEAN`. Columns are nullable unless declared `NOT NULL`; `NULL` is represented as its own typed value. A column may declare one literal `DEFAULT`, including an explicit `DEFAULT NULL`.

## Keys and constraints

Varchar supports one single-column primary key per table, any number of single-column UNIQUE constraints, and single-column foreign keys. Constraints may be written inline:

```sql
CREATE TABLE users (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  email TEXT UNIQUE
);

CREATE TABLE posts (
  id INTEGER PRIMARY KEY,
  user_id INTEGER REFERENCES users(id)
    ON DELETE CASCADE ON UPDATE CASCADE,
  editor_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
  body TEXT NOT NULL
);
```

The equivalent table-level forms include `PRIMARY KEY (id)`, `UNIQUE (email)`, and `FOREIGN KEY (user_id) REFERENCES users(id)`. Composite key and UNIQUE constraints are not supported. A primary key implies `NOT NULL` and is unique across the table; one UNIQUE declaration on that same column is accepted and normalized away. A non-primary UNIQUE column rejects duplicate non-NULL values but permits multiple NULLs. Text equality remains case- and normalization-sensitive. A foreign key must reference an existing primary-key column with the same type; UNIQUE columns are not foreign-key targets. Foreign-key columns remain nullable unless they also use `NOT NULL`; a `NULL` value does not need a matching parent row.

Key and CHECK constraints are enforced when data is inserted or updated and when a persisted database is loaded. Candidate validation checks primary keys and sequence state first, then UNIQUE, CHECK, and foreign keys. Foreign keys default to `ON DELETE RESTRICT ON UPDATE RESTRICT`; explicit `RESTRICT` is also accepted. `RESTRICT` rejects a parent mutation while a child row the statement does not itself name still holds that key; a child the same statement deletes, or rewrites off the old key, releases the restriction, and the reference it lands on is checked against the candidate database like any other. Deletes additionally support `ON DELETE CASCADE` and `ON DELETE SET NULL`; referenced-key updates support `ON UPDATE CASCADE`. Cascades may be multi-level, self-referential, or cyclic. Update cascades merge compatible changes to the same row, reject conflicting values for one column, and advance every affected auto-increment high-water mark. `SET NULL` requires a nullable foreign-key column. Cascaded and rewritten child rows are not included in the direct affected-row count. Like every failed mutation, a constraint violation leaves the authoritative string unchanged.

## Auto-increment keys

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

## Defaults

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

## CHECK constraints

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

## WHERE clauses

`WHERE` supports parentheses plus `AND`/`OR`, with `AND` binding more tightly than `OR`. Predicate leaves are:

- `=`, `!=`, `<`, `<=`, `>`, `>=`
- `IN (literal, ...)`, with at least one literal
- `LIKE`, where `%` matches any sequence and `_` matches one Unicode scalar
- `IS NULL`, `IS NOT NULL`

Backslash escapes `%`, `_`, and backslash inside a `LIKE` pattern. Comparisons, `IN`, and `LIKE` use SQL three-valued truth for nullable columns: a `NULL` left value produces unknown, and `WHERE` retains only true. Direct comparison to a `NULL` literal is a type error for every comparison operator; use `IS NULL` or `IS NOT NULL`. Ordered operands must have the column's type: integers use signed numeric order, booleans use `FALSE < TRUE`, and text uses case-sensitive decoded Unicode-scalar order without normalization. Every non-`NULL` `IN` member must have the column's type; an all-`NULL` list is valid for every column type, and duplicate members are preserved without changing the result. For a non-`NULL` left value, `IN` returns true for any equal non-`NULL` member, otherwise unknown when the list contains `NULL`, and otherwise false; a `NULL` left value is always unknown. Every list member is resolved left to right before execution, even when an earlier member matches. All leaves are resolved and type-checked before execution, even when runtime short-circuiting would skip one. Keywords and ASCII identifiers, whether quoted or unquoted, are case-insensitive. Text values and `LIKE` matching are case-sensitive.

## Joins

`SELECT` supports inner equijoins using either `JOIN` or `INNER JOIN`:

```sql
SELECT users.name, posts.body
FROM users
INNER JOIN posts ON users.id = posts.user_id
WHERE posts.body LIKE 'A%';
```

An `ON` clause contains column-to-column equality terms joined by `AND`. Additional join clauses form a left-to-right chain, and later clauses may refer to any earlier source. Column references in projections, `ON`, and `WHERE` may be table-qualified; a bare column is accepted only when exactly one participating table contains that name. `table.*` expands one table in schema order, while unqualified `*` expands all sources in `FROM`/`JOIN` order.

Join equality uses SQL null semantics: `NULL` never equals any value, including another `NULL`. Duplicate and many-to-many matches are preserved. Without ordering, results use deterministic nested-loop order: physical row order from the `FROM` table, followed by physical row order from each joined table left to right.

## Ordering

`ORDER BY` accepts one or more real source columns, each optionally followed by `ASC` or `DESC`. Sort columns need not be projected, duplicate terms are retained, and joined columns may be qualified only with their real table name. Unqualified names must be unambiguous. Ordering is lexicographic across INTEGER, BOOLEAN (`FALSE < TRUE`), and decoded TEXT Unicode scalars. Ascending order puts NULL after non-NULL values; descending order puts NULL first. Final ties preserve the same physical or nested-loop order an unordered query would have produced.

```sql
SELECT children.name
FROM parents JOIN children ON parents.id = children.parent_id
ORDER BY parents.created_at, children.name DESC;
```

Aliases, ordinals, expressions, `COLLATE`, and configurable NULL placement are not supported as sort terms.

## Pagination

A `SELECT` tail has the fixed shape `[ORDER BY ...] [LIMIT unsigned_integer] [OFFSET unsigned_integer]`. `OFFSET` is valid without `LIMIT`; both accept zero, leading zeroes, and values through `18446744073709551615`. Filtering and joins happen first, then ordering, then offset skipping, then limiting. An offset beyond the result cardinality returns the normal column metadata with no rows. Without `ORDER BY`, skipped rows are not cloned into the result and execution stops as soon as the limit is filled. An ordered query with a `LIMIT` retains only the `OFFSET + LIMIT` rows that could still land in the window and drops the rest as it scans; without a `LIMIT` it retains every qualifying row. `LIMIT 0` still parses, resolves, validates, generates, and compiles the query plan and charges output-column metadata, but it performs no row scan, join comparisons, ordered-row retention, or sorting.

```sql
SELECT * FROM events LIMIT 25;
SELECT * FROM events OFFSET 100;
SELECT * FROM events ORDER BY created_at DESC LIMIT 25 OFFSET 50;
```

## Result columns

Each library result column includes its display label and the table/column it originated from. When a joined result contains the same label from different sources, the CLI qualifies those headers with their table names.

Unconstrained tables retain duplicate rows. Projection order, duplicate projected columns, and physical insertion order are preserved.

## Unsupported syntax

The intentionally small dialect does not include outer joins, aliases, self-joins, aggregation, subqueries, unary `NOT`, comments, statement batches, or schema alteration. Double-quoted identifiers are accepted only when their contents use the same ASCII letter, digit, and underscore grammar as unquoted identifiers; they disambiguate reserved words but do not introduce case-sensitive names. Unsupported syntax is rejected rather than partially interpreted.

