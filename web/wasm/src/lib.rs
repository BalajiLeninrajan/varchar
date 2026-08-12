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
                    Outcome::Affected { rows } => json!({ "kind": "affected", "rows": rows }),
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
