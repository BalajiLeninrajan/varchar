// The engine. One `Db` lives in WebAssembly for the lifetime of the tab; every
// call returns a JSON envelope describing what happened plus the authoritative
// string as it stands afterwards.

import init, { Db } from "../wasm/varchar.js";
import wasmUrl from "../wasm/varchar_bg.wasm?url";

/** Boots the WebAssembly module and returns an empty database. */
export async function createDb() {
  await init({ module_or_path: wasmUrl });
  return new Db();
}

export function exec(db, sql) {
  return JSON.parse(db.exec(sql));
}

export function explain(db, sql) {
  return JSON.parse(db.explain(sql));
}

export function load(db, blob) {
  return JSON.parse(db.load(blob));
}

/** Splits console input into statements, honouring '' and "" quoting. */
export function splitStatements(text) {
  const statements = [];
  let current = "";
  let quote = null;
  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];
    if (quote) {
      current += character;
      if (character === quote) {
        if (text[index + 1] === quote) {
          current += quote;
          index += 1;
        } else {
          quote = null;
        }
      }
      continue;
    }
    if (character === "'" || character === '"') {
      quote = character;
      current += character;
    } else if (character === ";") {
      if (current.trim()) statements.push(current.trim());
      current = "";
    } else {
      current += character;
    }
  }
  if (current.trim()) statements.push(current.trim());
  return statements;
}

/** One-line summary of an envelope, for the log. */
export function describe(envelope) {
  switch (envelope.kind) {
    case "rows":
      return `${envelope.result.rows.length} row(s)`;
    case "affected":
      return `${envelope.rows} row(s) affected`;
    case "created":
      return `created ${envelope.table}`;
    case "explain":
      return `pattern compiled (${envelope.scan.matchCount ?? 0} matches)`;
    case "loaded":
      return "database loaded";
    default:
      return "ok";
  }
}
