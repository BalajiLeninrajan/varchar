// Reads the database string as a tape: one entry per record, each with the
// byte span it occupies, and what a scan did to it. Offsets are UTF-8 bytes,
// the same unit the engine reports its match ranges in.

import { decodeRange } from "./bytes.js";

const SEMICOLON = 0x3b;

/**
 * Splits the encoded string into records. Every record ends in `;`, and a
 * cell escapes `;` as `%00003B`, so the first `;` after a record's start is
 * always its end.
 */
export function splitRecords(bytes) {
  const records = [];
  let at = 0;
  while (at < bytes.length) {
    let end = bytes.indexOf(SEMICOLON, at);
    end = end === -1 ? bytes.length : end + 1;
    const text = decodeRange(bytes, at, end);
    const fields = text.slice(0, -1).split("|");
    const tag = text[0] === "~" ? text[1] : "";
    records.push({
      at,
      end,
      text,
      kind: tag === "R" ? "row" : tag === "S" ? "schema" : "meta",
      table: tag ? decodeText(fields[1] ?? "") : null,
      fields,
    });
    at = end;
  }
  return records;
}

/** Undoes the `%XXXXXX` scalar escapes the storage format writes into text. */
export function decodeText(raw) {
  return raw.replace(/%([0-9A-F]{6})/g, (_, hex) => String.fromCodePoint(parseInt(hex, 16)));
}

/** One stored cell, in the same shape the adapter uses for result values. */
export function decodeCell(field) {
  const tag = field[0];
  const body = field.slice(1);
  if (tag === "N") return { t: "null" };
  if (tag === "I") return { t: "integer", v: body };
  if (tag === "B") return { t: "boolean", v: body === "1" };
  return { t: "text", v: decodeText(body) };
}

const sameValue = (a, b) => a.t === b.t && (a.t === "null" || a.v === b.v);

/** Column names per table, in stored order, read from the `~S` records. */
export function schemas(records) {
  const tables = new Map();
  for (const record of records) {
    if (record.kind !== "schema") continue;
    tables.set(
      record.table,
      record.fields.slice(2).map((column) => decodeText(column.split(":")[0])),
    );
  }
  return tables;
}

/**
 * Runs of neighbouring records that belong together: the header and schema
 * records, then each table's rows. Drawn as brackets over the byte map.
 */
export function regions(records) {
  const runs = [];
  for (const record of records) {
    const label = record.kind === "row" ? record.table : "header and schema";
    const last = runs.at(-1);
    if (last && last.label === label) last.end = record.end;
    else runs.push({ label, at: record.at, end: record.end });
  }
  return runs;
}

/** Ruler ticks at a round step of at least ten bytes, at most eight of them. */
export function ticks(total) {
  if (total <= 0) return [];
  const rough = total / 8;
  const power = 10 ** Math.floor(Math.log10(rough));
  const step = Math.max(10, [1, 2, 5, 10].map((m) => m * power).find((s) => s >= rough));
  // A tick too close to the end would collide with the length label.
  const marks = [0];
  for (let at = step; at < total - step * 0.4; at += step) marks.push(at);
  return marks;
}

/**
 * What a scan did to each record, and which records each result row came from.
 *
 * States: `lit` when the pattern matched it and it reached the result, `half`
 * when the pattern matched it but Rust dropped it afterwards (a residual
 * predicate, the ON of a join, or LIMIT), `tested` for a row of a scanned
 * table the pattern rejected, and otherwise the record's own kind.
 *
 * The engine reports no row provenance, so rows are traced back by value:
 * each result column names its origin table and column, and the stored cells
 * of a matched record either agree with the row or they don't. When a row
 * can't be traced, or its cells fit more than one record, the whole mapping
 * is dropped, and matched records stay plainly lit rather than guessed at.
 */
export function readScan(records, scan, result) {
  const matches = scan?.matches ?? [];
  const sources = scan?.sources ?? [];
  const matched = new Set();
  let cursor = 0;
  for (const match of matches) {
    while (cursor < records.length && records[cursor].end <= match.start) cursor += 1;
    for (let index = cursor; index < records.length && records[index].at < match.end; index += 1) {
      matched.add(index);
    }
  }

  const state = records.map((record, index) =>
    matched.has(index) ? "lit" : record.kind === "row" && sources.includes(record.table) ? "tested" : record.kind,
  );

  const traced = result ? trace(records, matched, sources, result) : null;
  if (traced) {
    const used = new Set(traced.flat());
    const judged = new Set(traced.judged);
    matched.forEach((index) => {
      if (!used.has(index) && judged.has(records[index].table)) state[index] = "half";
    });
  }
  return { state, rows: traced };
}

function trace(records, matched, sources, { columns, rows }) {
  if (new Set(sources).size !== sources.length) return null;
  const tables = schemas(records);
  // For each source table: which result columns it supplies, and where each
  // of those sits in the table's stored record.
  const plan = sources
    .map((table) => {
      const order = tables.get(table) ?? [];
      const picks = columns
        .map((column, position) => ({ position, field: order.indexOf(column.column) + 2, table: column.table }))
        .filter((pick) => pick.table === table);
      return { table, picks };
    });
  // A source that supplies no result column can't be traced, and a partial
  // trace would pin a join row on a single record.
  if (plan.some((entry) => entry.picks.length === 0)) return null;
  if (plan.some((entry) => entry.picks.some((pick) => pick.field < 2))) return null;

  const pool = new Map(plan.map((entry) => [entry.table, [...matched].filter((i) => records[i].table === entry.table)]));
  const single = sources.length === 1;
  const taken = new Set();
  const traced = [];
  for (const row of rows) {
    const found = [];
    for (const { table, picks } of plan) {
      const fits = pool.get(table).filter((candidate) =>
        picks.every((pick) => {
          const field = records[candidate].fields[pick.field];
          return field !== undefined && sameValue(decodeCell(field), row[pick.position]);
        }),
      );
      // Two records that agree on every projected cell can't be told apart,
      // so the trace would be a guess. Give up and leave the matches lit.
      if (fits.length !== 1) return null;
      const index = fits[0];
      if (single && taken.has(index)) return null;
      found.push(index);
    }
    // One table's rows each become at most one result row. A join repeats
    // a record for every partner it pairs with.
    if (single) taken.add(found[0]);
    traced.push(found);
  }
  traced.judged = plan.map((entry) => entry.table);
  return traced;
}

/**
 * The scan's curve, cubic-bezier(.25, .2, .75, .8): near constant, slope .8
 * at the ends and about 1.1 through the middle. `reach(p)` is the fraction of
 * the scan's duration at which the head passes fraction `p` of the tape, so
 * a record can light the moment the head gets to it.
 */
export const READ_CURVE = [0.25, 0.2, 0.75, 0.8];

export function reach(p) {
  const [x1, y1, x2, y2] = READ_CURVE;
  const bezier = (a, c, s) => 3 * a * s * (1 - s) ** 2 + 3 * c * s * s * (1 - s) + s ** 3;
  let lo = 0;
  let hi = 1;
  for (let step = 0; step < 30; step += 1) {
    const s = (lo + hi) / 2;
    if (bezier(y1, y2, s) < p) lo = s;
    else hi = s;
  }
  return bezier(x1, x2, (lo + hi) / 2);
}
