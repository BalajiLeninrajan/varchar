// The dialect, clause by clause — a condensed docs/sql-reference.md, kept here
// so the playground can answer "what can I type?" without leaving the tab.
//
// An entry marked `run` is a complete statement the console can execute; those
// name the demo schema from presets.js, so seeding first makes every one of
// them work. Everything else is a fragment shown for its shape alone.

export const SECTIONS = [
  {
    title: "STATEMENTS",
    blurb: "One statement at a time, with an optional trailing semicolon. The console splits on ; and runs them in order.",
    entries: [
      {
        syntax: "CREATE TABLE jobs (id INTEGER PRIMARY KEY AUTOINCREMENT, state TEXT NOT NULL DEFAULT 'queued')",
        note: "Columns and constraints in one shot. There is no ALTER TABLE afterwards.",
        run: true,
      },
      {
        syntax: "INSERT INTO jobs VALUES (1, 'running')",
        note: "Positional: exactly one value per column, in declaration order.",
        run: true,
      },
      {
        syntax: "INSERT INTO jobs (state) VALUES ('queued')",
        note: "By name: an omitted column takes its DEFAULT, or its generated key.",
        run: true,
      },
      { syntax: "SELECT * FROM users", note: "* expands every source in FROM/JOIN order.", run: true },
      {
        syntax: "SELECT name, email FROM users WHERE active = TRUE",
        note: "A named projection. Duplicate and unprojected-but-sorted columns are both fine.",
        run: true,
      },
      {
        syntax: "UPDATE users SET active = FALSE WHERE id = 1",
        note: "A scan finds the rows, then the whole string is rewritten.",
        run: true,
      },
      {
        syntax: "DELETE FROM posts WHERE title LIKE 'Draft%'",
        note: "Same scan, and any ON DELETE action fires afterwards.",
        run: true,
      },
      { syntax: "SHOW TABLES", note: "The catalog, in creation order.", run: true },
      { syntax: "DESCRIBE users", note: "One row per column: type, nullability, keys, default.", run: true },
      {
        syntax: "SHOW CREATE TABLE users",
        note: "Canonical DDL rebuilt from the string, not the text you typed.",
        run: true,
      },
      {
        syntax: "EXPLAIN REGEX SELECT name FROM users WHERE active = TRUE",
        note: "Compiles the pattern without running the query.",
        run: true,
      },
    ],
  },
  {
    title: "COLUMN TYPES",
    blurb: "Three types, and every column is nullable unless it says otherwise.",
    entries: [
      { syntax: "TEXT", note: "UTF-8. Comparison and LIKE are case- and normalization-sensitive." },
      { syntax: "INTEGER", note: "Signed 64-bit." },
      { syntax: "BOOLEAN", note: "TRUE or FALSE, ordered FALSE < TRUE." },
      {
        syntax: "NULL",
        note: "A typed value of its own, not a missing one. Only IS NULL and IS NOT NULL test for it.",
      },
    ],
  },
  {
    title: "CONSTRAINTS",
    blurb: "Inline on the column, or table-level at the end of the CREATE. Checked on insert, on update, and again when a string is imported.",
    entries: [
      { syntax: "NOT NULL", note: "The column rejects NULL." },
      {
        syntax: "DEFAULT 'queued'",
        note: "One literal per column, applied only when a named-column insert omits it. An explicit NULL never triggers it.",
      },
      { syntax: "PRIMARY KEY", note: "One single-column key per table. Implies NOT NULL and uniqueness." },
      {
        syntax: "AUTOINCREMENT",
        note: "On an INTEGER PRIMARY KEY only; AUTO_INCREMENT is the same word. The high-water mark advances past larger keys and never falls.",
      },
      { syntax: "UNIQUE", note: "Rejects duplicate non-NULL values; several NULLs are fine." },
      {
        syntax: "CHECK (views >= 0)",
        note: "Table-local, and may name any column of the table, including one declared later. Rejects only false — unknown passes.",
      },
      {
        syntax: "REFERENCES users(id)",
        note: "One column onto a primary key of the same type. Defaults to ON DELETE RESTRICT ON UPDATE RESTRICT.",
      },
      {
        syntax: "ON DELETE RESTRICT | CASCADE | SET NULL",
        note: "SET NULL needs a nullable column. Cascades may be multi-level, self-referential, or cyclic.",
      },
      { syntax: "ON UPDATE RESTRICT | CASCADE", note: "An update cascade rewrites the children's keys." },
      {
        syntax: "PRIMARY KEY (id), UNIQUE (email), FOREIGN KEY (user_id) REFERENCES users(id)",
        note: "The table-level spellings of the same three. Composite keys are not supported.",
      },
    ],
  },
  {
    title: "WHERE",
    blurb: "Parentheses, AND and OR over the leaves below, with AND binding tighter. What can be pushed down becomes the pattern; the rest is re-checked in Rust after the scan.",
    entries: [
      {
        syntax: "= != < <= > >=",
        note: "The literal must have the column's type: INTEGER is numeric, BOOLEAN is FALSE < TRUE, TEXT is Unicode-scalar order.",
      },
      {
        syntax: "state IN ('queued', 'running')",
        note: "At least one literal. NULL members keep SQL truth, so x IN (NULL) is unknown rather than false.",
      },
      {
        syntax: "name LIKE 'A%'",
        note: "% matches any sequence, _ matches one Unicode scalar, and a backslash escapes %, _ and itself.",
      },
      { syntax: "email IS NULL / email IS NOT NULL", note: "The only NULL test: = NULL is a type error." },
      {
        syntax: "SELECT title, views FROM posts WHERE published = TRUE AND views > 1000",
        note: "Both leaves are resolved before execution; here the range cannot be pushed down, so the pattern is a prefilter.",
        run: true,
      },
    ],
    note: "A NULL operand makes a leaf unknown, and WHERE keeps only what is true. There is no unary NOT.",
  },
  {
    title: "JOINS",
    blurb: "Inner equijoins. JOIN and INNER JOIN are the same thing.",
    entries: [
      {
        syntax: "SELECT users.name, posts.title FROM users JOIN posts ON users.id = posts.user_id",
        note: "One pattern alternates over both tables; the ON equality is checked afterwards.",
        run: true,
      },
      {
        syntax: "ON a.x = b.x AND a.y = b.y",
        note: "Column-to-column equality terms joined by AND. NULL never equals anything, including another NULL.",
      },
      {
        syntax: "users.*",
        note: "Expands one table in schema order. A bare column name is accepted only when exactly one source has it.",
      },
    ],
    note: "Further JOIN clauses chain left to right and may refer to any earlier source. Duplicate and many-to-many matches are kept.",
  },
  {
    title: "ORDER AND PAGE",
    blurb: "The tail has one fixed shape: [ORDER BY ...] [LIMIT n] [OFFSET n].",
    entries: [
      {
        syntax: "SELECT title, views FROM posts ORDER BY views DESC LIMIT 3 OFFSET 1",
        note: "Filtering and joins first, then ordering, then the skip, then the limit.",
        run: true,
      },
      {
        syntax: "ORDER BY views DESC, title",
        note: "Real source columns, each with an optional ASC or DESC. Ascending puts NULL last, descending first.",
      },
      {
        syntax: "LIMIT 25 OFFSET 50",
        note: "Unsigned integers through 18446744073709551615. OFFSET is valid without LIMIT.",
      },
    ],
  },
  {
    title: "NOT IN THE DIALECT",
    blurb: "Rejected outright rather than partially interpreted.",
    items: [
      "outer joins",
      "aliases (AS)",
      "self-joins",
      "aggregates, GROUP BY",
      "subqueries",
      "unary NOT",
      "expressions in projections",
      "comments",
      "statement batches",
      "ALTER TABLE, DROP TABLE",
      "composite keys",
    ],
  },
];
