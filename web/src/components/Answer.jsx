// The answer to the last statement as one sentence, and the pattern that
// produced it in the tilted aside.

import { Fragment } from "preact";

import { Banner, CopyButton, EmptyState } from "./ui.jsx";
import { tokenizePattern } from "../lib/pattern.js";

// A no-break space keeps each count on the line with its noun when the
// headline wraps.
const plural = (n, word) => `${n.toLocaleString()}\u00a0${word}${n === 1 ? "" : "s"}`;

/** What a single-table scan read: "posts" for a table named posts, else records. */
function noun(scan, n) {
  const table = scan.sources.length === 1 ? scan.sources[0] : "";
  if (!/^[a-z]+s$/.test(table)) return plural(n, "record");
  return `${n.toLocaleString()}\u00a0${n === 1 ? table.slice(0, -1) : table}`;
}
const verb = (statement) => statement.trim().split(/\s+/)[0]?.toLowerCase() ?? "";

/** [headline parts, lede]. A headline part in an array is the emphasised noun. */
function sentence({ booted, outcome, reading }) {
  const bytes = `${reading.total.toLocaleString()}-byte`;
  if (!booted) return [["Starting the engine."], "The varchar crate is loading as WebAssembly."];
  if (!outcome) {
    return [
      ["Your database is a ", [bytes], " string."],
      reading.total <= 3
        ? "It holds only the header. Seed the demo data or run a CREATE TABLE."
        : "Run a SELECT to see which of its records the pattern lights.",
    ];
  }
  const { statement, envelope } = outcome;
  if (!envelope.ok) {
    return [["The engine turned that statement down."], "The string is unchanged, byte for byte. A failed statement never touches it."];
  }
  const { lit, scan } = reading;
  const kind = envelope.kind;

  if (kind === "rows" && scan) {
    const kept = envelope.result.rows.length;
    if (lit === 0) return [["Your query lit ", ["nothing"], " in a ", [bytes], " string."], `The pattern tested ${plural(reading.tested, "record")} and matched none of them.`];
    if (scan.sources.length > 1) {
      return [["Your join lit ", [plural(lit, "record")], " and paired them into ", [plural(kept, "row")], "."], "One alternation reads both tables, and Rust does the ON."];
    }
    if (scan.exact && kept === lit) {
      return [["Your query lit ", [plural(lit, "row")], " in a ", [bytes], " string."], "Every predicate is in the pattern, so each lit record is one row below."];
    }
    const dropped = lit - kept;
    return [
      ["Your query lit ", [noun(scan, lit)], " and kept ", [kept.toLocaleString()], "."],
      scan.exact
        ? `Every predicate is in the pattern, and Rust's LIMIT or OFFSET dropped ${dropped.toLocaleString()}.`
        : `Rust re-checked what the pattern can't hold and dropped ${dropped.toLocaleString()}.`,
    ];
  }
  if (kind === "rows") {
    return [["Your statement printed ", [plural(envelope.result.rows.length, "row")], " without a scan."], "Only a SELECT compiles to a pattern, so nothing on the tape is lit."];
  }
  if (kind === "explain") {
    return [["The pattern would light ", [plural(lit, "record")], "."], "EXPLAIN REGEX stops at the plan, so no rows were read out."];
  }
  if (kind === "affected" && scan) {
    const done = verb(statement) === "delete" ? "removed" : "rewrote";
    return [
      [`Your ${verb(statement)} lit `, [plural(lit, "row")], ` and ${done} `, [envelope.rows.toLocaleString()], "."],
      reading.historic ? "The tape shows the string as the scan read it, before the write." : "The tape now shows the string after the write, so nothing is lit.",
    ];
  }
  if (kind === "affected") {
    return [[`Your ${verb(statement) || "statement"} wrote `, [plural(envelope.rows, "row")], ". The string is now ", [plural(reading.total, "byte")], "."], "Every write re-encodes the whole string."];
  }
  if (kind === "created") {
    return [["Table ", [envelope.table], " is now a ~S record."], "Its schema lives in the string, ahead of the rows it will hold."];
  }
  if (kind === "loaded") {
    return [["You loaded a ", [bytes], " string."], "The engine validated every record before adopting it."];
  }
  return [["Done."], ""];
}

export function Answer(props) {
  const [headline, lede] = sentence(props);
  return (
    <header class="vc-answer">
      <h1 class="cn-display cn-m-0" aria-live="polite">
        {headline.map((part, index) => (Array.isArray(part) ? <em key={index}>{part[0]}</em> : part))}
      </h1>
      {lede ? <p class="lede">{lede}</p> : null}
    </header>
  );
}

/** Why there is no pattern to show. */
function missing(outcome) {
  if (!outcome) return ["No pattern yet", "Run a SELECT and the pattern the planner compiled appears here."];
  if (!outcome.envelope.ok) return ["No pattern", "The statement was rejected before anything was scanned."];
  return ["No pattern for this statement", "Only SELECT, UPDATE and DELETE compile to one."];
}

export function ReadHead({ outcome, reading }) {
  const { scan } = reading;
  const envelope = outcome?.envelope;
  let body;
  if (!scan) {
    const [title, text] = missing(outcome);
    body = <EmptyState fill={false} title={title}>{text}</EmptyState>;
  } else {
    const kept = envelope?.kind === "rows" ? envelope.result.rows.length : null;
    body = (
      <>
        <pre class="codeblock vc-pattern">
          {tokenizePattern(scan.pattern).map((token, index) => (
            <span key={index} class={`re-${token.kind}`}>
              {token.text}
            </span>
          ))}
        </pre>
        <dl class="kv">
          <dt>Tested</dt>
          <dd>
            {reading.tested.toLocaleString()}{" "}
            {scan.sources.map((table, index) => (
              <Fragment key={table}>
                {index ? ", " : ""}
                <code class="cn-code-inline">~R|{table}</code>
              </Fragment>
            ))}{" "}
            records
          </dd>
          <dt>Matched</dt>
          <dd>
            {plural(reading.lit, "record")}, {reading.litBytes.toLocaleString()} of {reading.total.toLocaleString()} bytes
          </dd>
          <dt>{envelope?.kind === "affected" ? "Changed" : "Returned"}</dt>
          <dd>
            {envelope?.kind === "affected"
              ? plural(envelope.rows, "row")
              : kept === null
                ? "nothing, EXPLAIN stops at the plan"
                : `${plural(kept, "row")}${scan.exact ? "" : ", after Rust re-checked each"}`}
          </dd>
        </dl>
        {scan.truncated ? <Banner>Only the first 4096 matches were collected for display.</Banner> : null}
        {scan.scanError ? <Banner>Replaying the pattern for display failed: {scan.scanError}</Banner> : null}
      </>
    );
  }

  return (
    <aside class="panel is-tilted vc-aside" aria-labelledby="head-heading" data-density="compact">
      <div class="panel-header">
        <h2 id="head-heading">The read head</h2>
        {scan ? (
          <span class="cn-row cn-gap-4">
            <span
              class="tag vc-tag"
              title={
                scan.exact
                  ? "Every predicate is in the pattern, so the matches are the result rows"
                  : "Residual predicates and JOIN ... ON are re-checked in Rust after the scan"
              }
            >
              {scan.exact ? "exact" : "prefilter"}
            </span>
            <CopyButton text={scan.pattern} />
          </span>
        ) : null}
      </div>
      <div class="panel-body cn-stack cn-gap-12">{body}</div>
    </aside>
  );
}
