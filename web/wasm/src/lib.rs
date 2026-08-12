//! A browser adapter for the `varchar` core.
//!
//! The whole authoritative database is one `String`, so the adapter keeps that
//! string in WebAssembly memory, executes one statement per call, and hands the
//! page a JSON envelope describing what happened. Every `SELECT` also carries
//! the generated scan pattern and the byte ranges it matched in the blob, which
//! is the part of the engine this playground exists to show.

use fancy_regex::Regex;
use serde_json::{Value as Json, json};
use varchar::{
    Database, Error, Outcome, ResultColumn, RowSet, SelectExplanation, Value as SqlValue,
};
use wasm_bindgen::prelude::*;

/// Match spans are only ever drawn, so a generous cap keeps a pathological
/// pattern from serializing the whole blob twice over.
const MAX_MATCHES: usize = 4096;

/// A live database whose authoritative state is the encoded string it returns
/// from [`Db::dump`].
#[wasm_bindgen]
pub struct Db {
    inner: Database,
}

#[wasm_bindgen]
impl Db {
    /// Creates an empty database.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Database::new(),
        }
    }

    /// The authoritative encoded string.
    #[must_use]
    pub fn dump(&self) -> String {
        self.inner.as_str().to_owned()
    }

    /// Replaces the database with an empty one.
    pub fn reset(&mut self) {
        self.inner = Database::new();
    }

    /// Validates and adopts a persisted blob, leaving the current database
    /// untouched when the blob is rejected.
    ///
    /// Returns the same JSON envelope shape as [`Db::exec`].
    pub fn load(&mut self, blob: String) -> String {
        match Database::from_string(blob) {
            Ok(database) => {
                self.inner = database;
                envelope(json!({ "kind": "loaded" }), self.inner.as_str())
            }
            Err(error) => failure(&error, self.inner.as_str()),
        }
    }

    /// Executes one statement and reports the result as a JSON envelope.
    ///
    /// A failed statement leaves the blob byte-for-byte unchanged, and the
    /// envelope always carries the blob as it stands after the attempt.
    pub fn exec(&mut self, sql: &str) -> String {
        // An `UPDATE` or `DELETE` scans the string as it stands *before* the
        // write, so its explanation has to be compiled first: afterwards the
        // rows the pattern matched have been rewritten or removed. The
        // before-image travels with it, since it is the only string those match
        // ranges index.
        let mutation = mutation::as_select(sql).and_then(|select| {
            let explanation = self.inner.explain_select(&select).ok()?;
            let before = self.inner.as_str().to_owned();
            let scan = mutation_scan_json(&explanation, &before);
            Some((scan, before))
        });

        match self.inner.execute(sql) {
            Ok(outcome) => {
                let blob = self.inner.as_str();
                let payload = match outcome {
                    Outcome::Rows(rows) => {
                        // `SHOW TABLES` and `DESCRIBE` are tabular too, but only
                        // a `SELECT` compiles to a scan pattern; the explanation
                        // is simply absent for the rest.
                        let scan = self
                            .inner
                            .explain_select(sql)
                            .ok()
                            .map(|explanation| explanation_json(&explanation, blob));
                        json!({
                            "kind": "rows",
                            "result": row_set_json(&rows),
                            "scan": scan,
                        })
                    }
                    // The scan is attached only once the statement has
                    // succeeded. `UPDATE` resolves its assignments before it
                    // compiles the scan, so a rejected mutation is exactly the
                    // case where the rewritten `SELECT` would have described a
                    // statement that never ran.
                    Outcome::Affected { rows } => match mutation {
                        Some((scan, before)) => json!({
                            "kind": "affected",
                            "rows": rows,
                            "scan": scan,
                            "blobBefore": before,
                        }),
                        None => json!({ "kind": "affected", "rows": rows }),
                    },
                    Outcome::Created { table } => json!({ "kind": "created", "table": table }),
                    Outcome::Explain(explanation) => json!({
                        "kind": "explain",
                        "scan": explanation_json(&explanation, blob),
                    }),
                };
                envelope(payload, blob)
            }
            Err(error) => failure(&error, self.inner.as_str()),
        }
    }

    /// Compiles a `SELECT` to its scan pattern without executing it.
    pub fn explain(&self, sql: &str) -> String {
        let blob = self.inner.as_str();
        match self.inner.explain_select(sql) {
            Ok(explanation) => envelope(
                json!({ "kind": "explain", "scan": explanation_json(&explanation, blob) }),
                blob,
            ),
            Err(error) => failure(&error, blob),
        }
    }
}

impl Default for Db {
    fn default() -> Self {
        Self::new()
    }
}

fn envelope(mut payload: Json, blob: &str) -> String {
    if let Some(object) = payload.as_object_mut() {
        object.insert("ok".into(), Json::Bool(true));
        object.insert("blob".into(), Json::String(blob.to_owned()));
    }
    payload.to_string()
}

fn failure(error: &Error, blob: &str) -> String {
    json!({
        "ok": false,
        "blob": blob,
        "error": error_json(error),
    })
    .to_string()
}

fn error_json(error: &Error) -> Json {
    let mut object = json!({ "message": error.to_string() });
    let map = object
        .as_object_mut()
        .expect("the diagnostic envelope is an object");
    let (kind, detail) = match error {
        Error::Parse {
            span_start,
            span_end,
            ..
        } => ("parse", json!({ "start": span_start, "end": span_end })),
        Error::Unsupported {
            span_start,
            span_end,
            ..
        } => (
            "unsupported",
            json!({ "start": span_start, "end": span_end }),
        ),
        Error::Schema(_) => ("schema", Json::Null),
        Error::Type(_) => ("type", Json::Null),
        Error::Constraint(_) => ("constraint", Json::Null),
        Error::CorruptStorage { offset, .. } => ("corrupt", json!({ "offset": offset })),
        Error::RegexCompile(_) => ("regex-compile", Json::Null),
        Error::RegexRuntime(_) => ("regex-runtime", Json::Null),
        Error::ResourceLimit { resource, limit } => (
            "limit",
            json!({ "resource": resource.to_string(), "limit": limit }),
        ),
        Error::Allocation { .. } => ("allocation", Json::Null),
        Error::Capacity { .. } => ("capacity", Json::Null),
        _ => ("other", Json::Null),
    };
    map.insert("kind".into(), Json::String(kind.into()));
    if !detail.is_null() {
        map.insert("detail".into(), detail);
    }
    object
}

fn row_set_json(rows: &RowSet) -> Json {
    json!({
        "columns": rows.columns().iter().map(column_json).collect::<Vec<_>>(),
        "rows": rows
            .rows()
            .iter()
            .map(|row| row.iter().map(value_json).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
    })
}

fn column_json(column: &ResultColumn) -> Json {
    json!({
        "label": column.label(),
        "table": column.origin().table(),
        "column": column.origin().column(),
        "type": column.data_type().to_string(),
        "nullable": column.nullable(),
    })
}

fn value_json(value: &SqlValue) -> Json {
    match value {
        // Integers travel as text as well: the column is a signed 64-bit value
        // and JSON numbers lose precision past 2^53 in the page.
        SqlValue::Integer(number) => json!({ "t": "integer", "v": number.to_string() }),
        SqlValue::Text(text) => json!({ "t": "text", "v": text }),
        SqlValue::Boolean(flag) => json!({ "t": "boolean", "v": flag }),
        SqlValue::Null => json!({ "t": "null" }),
    }
}

fn explanation_json(explanation: &SelectExplanation, blob: &str) -> Json {
    let mut scan = json!({
        "pattern": explanation.pattern(),
        "exact": explanation.pattern_is_exact(),
        "sources": explanation.sources(),
        "columns": explanation.columns().iter().map(column_json).collect::<Vec<_>>(),
    });
    let map = scan.as_object_mut().expect("the scan report is an object");
    match matches_json(explanation.pattern(), blob) {
        Ok((matches, truncated)) => {
            map.insert("matchCount".into(), json!(matches.len()));
            map.insert("matches".into(), Json::Array(matches));
            map.insert("truncated".into(), Json::Bool(truncated));
        }
        // Replaying the pattern is a display convenience; the engine already
        // ran the real scan, so a replay failure annotates rather than fails.
        Err(message) => {
            map.insert("scanError".into(), Json::String(message));
        }
    }
    scan
}

/// The scan an `UPDATE` or `DELETE` performed.
///
/// `appliesTo` marks the ranges as indexing the string *before* the write. The
/// pattern itself stays valid afterwards, but the rows it matched have been
/// re-encoded or removed, so every byte past the first edit has shifted; the
/// only string these offsets describe is the one the scan actually read.
fn mutation_scan_json(explanation: &SelectExplanation, before: &str) -> Json {
    let mut scan = json!({
        "pattern": explanation.pattern(),
        "exact": explanation.pattern_is_exact(),
        "sources": explanation.sources(),
        "appliesTo": "before",
    });
    let map = scan.as_object_mut().expect("the scan report is an object");
    match matches_json(explanation.pattern(), before) {
        Ok((matches, truncated)) => {
            map.insert("matchCount".into(), json!(matches.len()));
            map.insert("matches".into(), Json::Array(matches));
            map.insert("truncated".into(), Json::Bool(truncated));
        }
        Err(message) => {
            map.insert("scanError".into(), Json::String(message));
        }
    }
    scan
}

fn matches_json(pattern: &str, blob: &str) -> Result<(Vec<Json>, bool), String> {
    let regex = Regex::new(pattern).map_err(|error| error.to_string())?;
    let mut matches = Vec::new();
    let mut truncated = false;
    for found in regex.find_iter(blob) {
        let found = found.map_err(|error| error.to_string())?;
        if matches.len() == MAX_MATCHES {
            truncated = true;
            break;
        }
        matches.push(json!({
            "start": found.start(),
            "end": found.end(),
        }));
    }
    Ok((matches, truncated))
}

/// Rewriting a mutation into the `SELECT` that performs the same row scan.
///
/// `UPDATE t SET ... WHERE p` and `DELETE FROM t WHERE p` compile their scan
/// through the same `pattern::row_scan_pattern` call the engine uses for a
/// single-source `SELECT`, from predicates resolved against the same table
/// schema, so `SELECT * FROM t WHERE p` explains the pattern the mutation
/// really ran. Projection never appears in a scan pattern, which is why `*`
/// costs nothing here.
///
/// The rewrite is deliberately conservative: anything it cannot tokenize
/// cleanly yields `None` and the statement simply reports no scan.
mod mutation {
    /// The dialect quotes text with `'` and identifiers with `"`, doubles the
    /// quote to escape it, and has no comment syntax. That makes a flat token
    /// scan enough to find clause keywords without mistaking one inside a
    /// literal for the real thing.
    #[derive(Debug, PartialEq, Eq)]
    enum Token {
        /// An unquoted run of identifier bytes: a keyword or a bare name.
        Word { start: usize, end: usize },
        /// A `"quoted identifier"`, including its quotes.
        Quoted { start: usize, end: usize },
        /// A `'text literal'`, including its quotes.
        Text,
        /// A single byte of punctuation.
        Punct,
    }

    fn tokenize(sql: &str) -> Option<Vec<Token>> {
        let bytes = sql.as_bytes();
        let mut tokens = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'\'' || byte == b'"' {
                let start = index;
                index += 1;
                loop {
                    // An unterminated quote is not a statement the engine would
                    // accept either, so refuse to guess at its shape.
                    let found = *bytes.get(index)?;
                    index += 1;
                    if found != byte {
                        continue;
                    }
                    if bytes.get(index) == Some(&byte) {
                        index += 1;
                        continue;
                    }
                    break;
                }
                tokens.push(if byte == b'"' {
                    Token::Quoted { start, end: index }
                } else {
                    Token::Text
                });
            } else if byte.is_ascii_alphanumeric() || byte == b'_' {
                let start = index;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                {
                    index += 1;
                }
                tokens.push(Token::Word { start, end: index });
            } else if byte.is_ascii_whitespace() {
                index += 1;
            } else {
                tokens.push(Token::Punct);
                index += 1;
            }
        }
        Some(tokens)
    }

    fn keyword<'sql>(sql: &'sql str, token: Option<&Token>) -> Option<&'sql str> {
        match token? {
            Token::Word { start, end } => Some(&sql[*start..*end]),
            _ => None,
        }
    }

    /// The `SELECT` whose scan matches this mutation's, or `None` for any other
    /// statement.
    pub(super) fn as_select(sql: &str) -> Option<String> {
        let tokens = tokenize(sql)?;
        let head = keyword(sql, tokens.first())?;

        if head.eq_ignore_ascii_case("delete") {
            // `DELETE FROM <tail>` and `SELECT * FROM <tail>` share the tail
            // grammar exactly, so this is a prefix splice with nothing to parse.
            let from = tokens.get(1)?;
            if !keyword(sql, Some(from))?.eq_ignore_ascii_case("from") {
                return None;
            }
            let Token::Word { start, .. } = from else {
                return None;
            };
            return Some(format!("SELECT * {}", &sql[*start..]));
        }

        if !head.eq_ignore_ascii_case("update") {
            return None;
        }
        let table = match tokens.get(1)? {
            Token::Word { start, end } | Token::Quoted { start, end } => &sql[*start..*end],
            _ => return None,
        };
        if !keyword(sql, tokens.get(2))?.eq_ignore_ascii_case("set") {
            return None;
        }
        // The engine's own parser ends the assignment list at the first WHERE
        // it meets, so finding the first top-level `where` word agrees with it
        // by construction.
        let clause = tokens.get(3..)?.iter().find_map(|token| match token {
            Token::Word { start, end } if sql[*start..*end].eq_ignore_ascii_case("where") => {
                Some(*start)
            }
            _ => None,
        });
        Some(match clause {
            Some(start) => format!("SELECT * FROM {} {}", table, &sql[start..]),
            None => format!("SELECT * FROM {table}"),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::as_select;

        #[test]
        fn rewrites_a_delete_by_splicing_its_tail() {
            assert_eq!(
                as_select("DELETE FROM users WHERE id = 1").as_deref(),
                Some("SELECT * FROM users WHERE id = 1"),
            );
            assert_eq!(
                as_select("delete from users").as_deref(),
                Some("SELECT * from users"),
            );
        }

        #[test]
        fn rewrites_an_update_by_dropping_its_assignments() {
            assert_eq!(
                as_select("UPDATE users SET active = FALSE WHERE id = 1").as_deref(),
                Some("SELECT * FROM users WHERE id = 1"),
            );
            assert_eq!(
                as_select("UPDATE users SET active = FALSE").as_deref(),
                Some("SELECT * FROM users"),
            );
        }

        #[test]
        fn a_literal_holding_a_clause_keyword_is_not_a_clause() {
            assert_eq!(
                as_select("UPDATE t SET a = 'p where b = 1', c = 'q' WHERE d = 2").as_deref(),
                Some("SELECT * FROM t WHERE d = 2"),
            );
            assert_eq!(
                as_select("UPDATE t SET note = 'set where' ").as_deref(),
                Some("SELECT * FROM t"),
            );
        }

        #[test]
        fn quoted_identifiers_survive_intact() {
            assert_eq!(
                as_select(r#"UPDATE "where" SET "set" = 'x' WHERE id = 1"#).as_deref(),
                Some(r#"SELECT * FROM "where" WHERE id = 1"#),
            );
        }

        #[test]
        fn a_doubled_quote_does_not_end_its_literal() {
            assert_eq!(
                as_select("UPDATE t SET a = 'it''s where' WHERE id = 1").as_deref(),
                Some("SELECT * FROM t WHERE id = 1"),
            );
        }

        #[test]
        fn other_statements_are_left_alone() {
            for sql in [
                "SELECT * FROM users",
                "INSERT INTO users (name) VALUES ('Ada')",
                "CREATE TABLE users (id INTEGER)",
                "SHOW TABLES",
                "DESCRIBE users",
                "EXPLAIN REGEX SELECT * FROM users",
                "",
            ] {
                assert_eq!(as_select(sql), None, "{sql:?} should not be rewritten");
            }
        }

        #[test]
        fn a_malformed_statement_is_refused_rather_than_guessed() {
            assert_eq!(as_select("UPDATE t SET a = 'unterminated WHERE id = 1"), None);
            assert_eq!(as_select("DELETE users WHERE id = 1"), None);
            assert_eq!(as_select("UPDATE t WHERE id = 1"), None);
            assert_eq!(as_select("UPDATE 'literal' SET a = 1"), None);
        }
    }
}
