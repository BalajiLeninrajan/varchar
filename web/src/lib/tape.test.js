import assert from "node:assert/strict";
import { test } from "node:test";

import { encode } from "./bytes.js";
import { decodeText, reach, readScan, regions, splitRecords, ticks } from "./tape.js";

// The demo database byte for byte, as the engine dumps it after the seed.
const BLOB =
  "V3;~S|users|id:I:!|name:T:!|email:T:?|active:B:?;~P|users|id;~A|users|id|I4;~D|users|active|B1;~U|users|email;~C|users|NE|1|T;~S|posts|id:I:!|user_id:I:?|title:T:!|views:I:?|published:B:?;~P|posts|id;~F|posts|user_id|users|id|C|R;~A|posts|id|I5;~D|posts|views|I0;~D|posts|published|B0;~R|users|I1|TAda Lovelace|Tada@example.com|B1;~R|users|I2|TAlan Turing|Talan@example.com|B1;~R|users|I3|TGrace Hopper|Tgrace@example.com|B0;~R|users|I4|TAnonymous|N|B1;~R|posts|I1|I1|TNotes on the Analytical Engine|I4120|B1;~R|posts|I2|I1|TA sketch of Bernoulli numbers|I890|B1;~R|posts|I3|I2|TOn computable numbers|I9001|B1;~R|posts|I4|I2|TDraft: the imitation game|I12|B0;~R|posts|I5|I3|TThe first compiler|I3300|B1;";

const records = splitRecords(encode(BLOB));
const text = (v) => ({ t: "text", v });
const int = (v) => ({ t: "integer", v });
const col = (table, column) => ({ table, column, label: column });
const spans = (...pairs) => pairs.map(([start, end]) => ({ start, end }));
const at = (indexes) => indexes.map((i) => records[i].at);

test("splits the string into records that cover every byte", () => {
  assert.equal(records[0].text, "V3;");
  assert.equal(records.at(-1).end, 703);
  records.forEach((r, i) => i && assert.equal(r.at, records[i - 1].end));
  assert.equal(records.filter((r) => r.kind === "row").length, 9);
});

test("brackets the schema and each table's rows", () => {
  assert.deepEqual(
    regions(records).map((r) => [r.label, r.at, r.end]),
    [["header and schema", 0, 285], ["users", 285, 453], ["posts", 453, 703]],
  );
});

test("an exact scan lights each returned row and marks the rest tested", () => {
  const scan = { sources: ["users"], matches: spans([285, 331], [331, 377], [425, 453]) };
  const result = {
    columns: [col("users", "name"), col("users", "email")],
    rows: [[text("Ada Lovelace"), text("ada@example.com")], [text("Alan Turing"), text("alan@example.com")], [text("Anonymous"), { t: "null" }]],
  };
  const { state, rows } = readScan(records, scan, result);
  const users = records.map((r, i) => [r, i]).filter(([r]) => r.table === "users" && r.kind === "row");
  assert.deepEqual(users.map(([, i]) => state[i]), ["lit", "lit", "tested", "lit"]);
  assert.deepEqual(rows.map(at), [[285], [331], [425]]);
});

test("a prefilter half-lights the records Rust dropped", () => {
  const scan = { sources: ["posts"], matches: spans([453, 509], [509, 563], [563, 610], [610, 659], [659, 703]) };
  const result = {
    columns: [col("posts", "title"), col("posts", "views")],
    rows: [[text("Notes on the Analytical Engine"), int("4120")], [text("On computable numbers"), int("9001")], [text("The first compiler"), int("3300")]],
  };
  const { state } = readScan(records, scan, result);
  const posts = state.filter((_, i) => records[i].table === "posts" && records[i].kind === "row");
  assert.deepEqual(posts, ["lit", "half", "lit", "half", "lit"]);
});

test("a join traces each row to both of its records", () => {
  const scan = { sources: ["users", "posts"], matches: spans([285, 703]) };
  const result = {
    columns: [col("users", "name"), col("posts", "title")],
    rows: [[text("Grace Hopper"), text("The first compiler")]],
  };
  const { rows, state } = readScan(records, scan, result);
  assert.deepEqual(rows.map(at), [[377, 659]]);
  assert.equal(state[records.findIndex((r) => r.at === 285)], "half");
});

test("rows that don't identify their record leave every match lit", () => {
  // SELECT published FROM posts WHERE views > 1000: four posts are published,
  // so a true value could have come from any of them.
  const scan = { sources: ["posts"], matches: spans([453, 509], [509, 563], [563, 610], [610, 659], [659, 703]) };
  const yes = { t: "boolean", v: true };
  const result = { columns: [col("posts", "published")], rows: [[yes], [yes], [yes]] };
  const { rows, state } = readScan(records, scan, result);
  assert.equal(rows, null);
  const posts = state.filter((_, i) => records[i].table === "posts" && records[i].kind === "row");
  assert.deepEqual(posts, ["lit", "lit", "lit", "lit", "lit"]);
});

test("a join that projects one table leaves every match lit", () => {
  const scan = { sources: ["users", "posts"], matches: spans([285, 703]) };
  const result = { columns: [col("users", "name")], rows: [[text("Grace Hopper")]] };
  const { rows, state } = readScan(records, scan, result);
  assert.equal(rows, null);
  assert.ok(state.every((s) => s !== "half"));
});

test("an untraceable row keeps every match plainly lit", () => {
  const scan = { sources: ["users"], matches: spans([285, 331]) };
  const result = { columns: [col("users", "name")], rows: [[text("Nobody")]] };
  const { rows, state } = readScan(records, scan, result);
  assert.equal(rows, null);
  assert.equal(state[records.findIndex((r) => r.at === 285)], "lit");
});

test("decodes scalar escapes", () => {
  assert.equal(decodeText("a%00003Bb%00007C"), "a;b|");
});

test("ruler ticks land on round numbers", () => {
  assert.deepEqual(ticks(703), [0, 100, 200, 300, 400, 500, 600]);
  assert.deepEqual(ticks(3), [0]);
  assert.deepEqual(ticks(25_000), [0, 5000, 10000, 15000, 20000]);
});

test("the head's timing follows its curve from end to end", () => {
  assert.ok(Math.abs(reach(0)) < 1e-6);
  assert.ok(Math.abs(reach(1) - 1) < 1e-6);
  assert.ok(Math.abs(reach(0.5) - 0.5) < 1e-6);
  assert.ok(reach(0.25) < 0.3 && reach(0.25) > 0.2);
});
