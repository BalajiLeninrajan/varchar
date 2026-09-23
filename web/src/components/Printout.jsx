// What the head printed: the result rows, each pointing back at the bytes on
// the tape it was read from.

import { Banner, EmptyState } from "./ui.jsx";
import { decodeRange, encode } from "../lib/bytes.js";

const plural = (n, word) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;
const pct = (n, total) => `${((n / total) * 100).toFixed(3)}%`;

function Cell({ value, label }) {
  if (value.t === "null") {
    return (
      <td class="vc-null" data-label={label}>
        NULL
      </td>
    );
  }
  // Integers arrive as text because they are 64-bit, so BigInt groups them.
  const text =
    value.t === "boolean" ? (value.v ? "TRUE" : "FALSE") : value.t === "integer" ? BigInt(value.v).toLocaleString() : value.v;
  return <td data-label={label}>{text}</td>;
}

function Rows({ columns, rows, reading, onPoint }) {
  const { records, total } = reading;
  const traced = reading.rows;
  return (
    <table class="data-table">
      <thead>
        <tr>
          {traced ? <th>on the tape</th> : null}
          {columns.map((column, index) => (
            <th key={index}>{column.label}</th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map((row, rowIndex) => {
          const from = traced?.[rowIndex].map((index) => records[index]);
          const point = from ? () => onPoint(rowIndex) : undefined;
          return (
            <tr key={rowIndex} tabindex={from ? 0 : undefined} onPointerEnter={point} onFocus={point}>
              {from ? (
                <td class="vc-at" data-label="on the tape">
                  <span class="cn-meta cn-tabular">bytes {from.map((r) => `${r.at} to ${r.end}`).join(", ")}</span>
                  <span class="vc-mini" aria-hidden="true">
                    {from.map((r) => (
                      <i key={r.at} style={{ left: pct(r.at, total), width: pct(r.end - r.at, total) }} />
                    ))}
                  </span>
                </td>
              ) : null}
              {row.map((value, index) => (
                <Cell key={index} value={value} label={columns[index].label} />
              ))}
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

/** A parse or unsupported-syntax error carries a byte span into the input. */
function Offender({ statement, detail }) {
  const bytes = encode(statement);
  return (
    <pre class="codeblock vc-offender scroll-well">
      {decodeRange(bytes, 0, detail.start)}
      <mark>{decodeRange(bytes, detail.start, detail.end) || "⟨here⟩"}</mark>
      {decodeRange(bytes, detail.end, bytes.length)}
    </pre>
  );
}

const DONE = {
  affected: (e) => ["Nothing printed", `A write prints no rows. ${plural(e.rows, "row")} changed and the string was rewritten in full.`],
  created: (e) => ["Nothing printed", `Creating ${e.table} printed no rows. Its schema is now a ~S record on the tape.`],
  explain: () => ["Nothing printed", "EXPLAIN REGEX stops at the plan, so no rows were read out."],
  loaded: () => ["Nothing printed", "The string was loaded and validated. Run a SELECT against it."],
};

export function Printout({ booted, outcome, reading, onPoint }) {
  let meta = null;
  let body = (
    <EmptyState fill={false} title={booted ? "Nothing run yet" : "Starting"}>Rows, affected counts and errors land here.</EmptyState>
  );

  if (outcome) {
    const { statement, envelope } = outcome;
    if (!envelope.ok) {
      body = (
        <div class="panel-body cn-stack cn-gap-12">
          <Banner tone="red">{envelope.error.message}</Banner>
          {envelope.error.detail && typeof envelope.error.detail.start === "number" ? (
            <Offender statement={statement} detail={envelope.error.detail} />
          ) : null}
        </div>
      );
    } else if (envelope.kind === "rows") {
      const { columns, rows } = envelope.result;
      meta = `${plural(rows.length, "row")}, ${plural(columns.length, "column")}`;
      body =
        rows.length === 0 ? (
          <EmptyState fill={false} title="No rows">The scan ran and nothing it matched survived the filter.</EmptyState>
        ) : (
          <Rows columns={columns} rows={rows} reading={reading} onPoint={onPoint} />
        );
    } else {
      const [title, detail] = (DONE[envelope.kind] ?? (() => ["Done", ""]))(envelope);
      body = <EmptyState fill={false} title={title}>{detail}</EmptyState>;
    }
  }

  return (
    <section class="panel vc-rows" aria-labelledby="rows-heading" data-density="compact">
      <div class="panel-header">
        <h2 id="rows-heading">What the head printed</h2>
        {meta ? <span class="cn-meta">{meta}</span> : null}
      </div>
      {body}
    </section>
  );
}
