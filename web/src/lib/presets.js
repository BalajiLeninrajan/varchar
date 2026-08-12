// Preset statements. The dialect has no comments and no statement batches, so
// every entry is a list of complete statements the console runs one at a time.

export const DEMO = [
  "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, email TEXT UNIQUE, active BOOLEAN DEFAULT TRUE, CHECK (name != ''))",
  "CREATE TABLE posts (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER REFERENCES users(id) ON DELETE CASCADE, title TEXT NOT NULL, views INTEGER DEFAULT 0, published BOOLEAN DEFAULT FALSE)",
  "INSERT INTO users (name, email, active) VALUES ('Ada Lovelace', 'ada@example.com', TRUE)",
  "INSERT INTO users (name, email, active) VALUES ('Alan Turing', 'alan@example.com', TRUE)",
  "INSERT INTO users (name, email, active) VALUES ('Grace Hopper', 'grace@example.com', FALSE)",
  "INSERT INTO users (name, email) VALUES ('Anonymous', NULL)",
  "INSERT INTO posts (user_id, title, views, published) VALUES (1, 'Notes on the Analytical Engine', 4120, TRUE)",
  "INSERT INTO posts (user_id, title, views, published) VALUES (1, 'A sketch of Bernoulli numbers', 890, TRUE)",
  "INSERT INTO posts (user_id, title, views, published) VALUES (2, 'On computable numbers', 9001, TRUE)",
  "INSERT INTO posts (user_id, title, views) VALUES (2, 'Draft: the imitation game', 12)",
  "INSERT INTO posts (user_id, title, views, published) VALUES (3, 'The first compiler', 3300, TRUE)",
];

export const GROUPS = [
  {
    title: "SCHEMA",
    presets: [
      {
        id: "schema-users",
        name: "Table with every constraint",
        blurb: "PRIMARY KEY AUTOINCREMENT, UNIQUE, DEFAULT, CHECK",
        sql: [DEMO[0]],
      },
      {
        id: "schema-posts",
        name: "Child table with a foreign key",
        blurb: "REFERENCES users(id) ON DELETE CASCADE",
        sql: [DEMO[1]],
      },
    ],
  },
  {
    title: "DATA",
    presets: [
      {
        id: "seed-users",
        name: "Seed four users",
        blurb: "Named-column inserts, generated keys, one NULL email",
        sql: DEMO.slice(2, 6),
      },
      {
        id: "seed-posts",
        name: "Seed five posts",
        blurb: "Child rows pointing at the users above",
        sql: DEMO.slice(6),
      },
    ],
  },
  {
    title: "SELECT — WATCH THE REGEX",
    presets: [
      {
        id: "select-all",
        name: "Everything",
        blurb: "SELECT * FROM users",
        sql: ["SELECT * FROM users"],
      },
      {
        id: "select-boolean",
        name: "A boolean equality",
        blurb: "The whole predicate compiles into the pattern",
        sql: ["SELECT name, email FROM users WHERE active = TRUE"],
      },
      {
        id: "select-like",
        name: "A LIKE prefix",
        blurb: "% becomes a bounded wildcard inside the cell",
        sql: ["SELECT name FROM users WHERE name LIKE 'A%'"],
      },
      {
        id: "select-residual",
        name: "A range that cannot be pushed down",
        blurb: "Prefilter pattern, then Rust re-checks views > 1000",
        sql: ["SELECT title, views FROM posts WHERE views > 1000"],
      },
      {
        id: "select-and",
        name: "AND across two columns",
        blurb: "Both predicates land in one pattern",
        sql: ["SELECT title FROM posts WHERE published = TRUE AND user_id = 1"],
      },
      {
        id: "select-join",
        name: "A join",
        blurb: "Alternation over both tables, ON checked in Rust",
        sql: ["SELECT users.name, posts.title FROM users JOIN posts ON users.id = posts.user_id"],
      },
      {
        id: "select-page",
        name: "ORDER BY, LIMIT, OFFSET",
        blurb: "Sorting and paging happen after the scan",
        sql: ["SELECT title, views FROM posts ORDER BY views DESC LIMIT 3 OFFSET 1"],
      },
      {
        id: "select-explain",
        name: "EXPLAIN REGEX",
        blurb: "Compile the pattern without running the query",
        sql: ["EXPLAIN REGEX SELECT name FROM users WHERE email IS NOT NULL"],
      },
    ],
  },
  {
    title: "MUTATE",
    presets: [
      {
        id: "update",
        name: "Update a row",
        blurb: "A scan finds the row, then the whole string is rewritten",
        sql: ["UPDATE users SET active = FALSE WHERE name = 'Alan Turing'"],
      },
      {
        id: "delete-prefilter",
        name: "Delete on a range",
        blurb: "Prefilter again: more rows match than the query deletes",
        sql: ["DELETE FROM posts WHERE views > 1000"],
      },
      {
        id: "delete-cascade",
        name: "Delete a parent, cascade the children",
        blurb: "One scan finds the parent; the cascade uses no regex at all",
        sql: ["DELETE FROM users WHERE id = 1"],
      },
      {
        id: "constraint",
        name: "Break a constraint",
        blurb: "Rejected — the string stays byte-for-byte identical",
        sql: ["INSERT INTO users (name, email) VALUES ('Ada Again', 'ada@example.com')"],
      },
    ],
  },
  {
    title: "INTROSPECT",
    presets: [
      { id: "show-tables", name: "SHOW TABLES", blurb: "Read the catalog", sql: ["SHOW TABLES"] },
      { id: "describe", name: "DESCRIBE users", blurb: "Column types, keys, defaults", sql: ["DESCRIBE users"] },
      { id: "show-create", name: "SHOW CREATE TABLE users", blurb: "Canonical DDL rebuilt from the string", sql: ["SHOW CREATE TABLE users"] },
    ],
  },
];
