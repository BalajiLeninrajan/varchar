# varchar

> [!WARNING]
> **Work in progress:** `varchar` is an experimental toy database under active
> development. Its APIs, SQL dialect, storage format, and command-line
> interface may change without notice. Do not use it for production data.

`varchar` is a deliberately absurd database: its entire authoritative
state—schemas, constraints, sequence state, and rows—is stored in one UTF-8
`String`, and every supported `SELECT` scans that string with generated regular
expressions.

Despite the joke premise, the project includes a real SQL parser, type checker,
storage codec, constraint system, and query engine. The goal is to explore how
far the one-string constraint can be taken while keeping the implementation
understandable and inspectable.

## Project structure

The Cargo workspace has two crates:

- `varchar` contains the platform-neutral database engine.
- `varchar-cli` provides the native command-line interface and file
  persistence.

The core owns no filesystem or terminal APIs. Parsed schemas, syntax trees,
compiled regular expressions, and result rows may exist temporarily, but the
single encoded string remains the only authoritative database state.

`EXPLAIN REGEX` exposes the regular expression generated for a query, making
the central trick visible rather than hiding it behind the engine.

## Current status

The project is still taking shape. Features, documentation, compatibility, and
data-format stability are not guaranteed yet. The source and test suites are
the most accurate description of currently supported behavior.

Contributions and experiments are welcome, with the expectation that things
will move and break while the design settles.
