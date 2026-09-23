// The database string drawn as a byte map, shown in the string dock above
// the string itself: one span per record, grouped under
// a bracket per table, with a byte ruler below. After a scan the read head
// lights each matched record in turn, left to right, the order a regex reads.

import { Fragment } from "preact";

import { reach, regions, ticks } from "../lib/tape.js";

/** Seconds the lighting takes to sweep the tape, left to right. */
const SCAN_SECONDS = 1.6;

const pct = (n, total) => `${((n / total) * 100).toFixed(3)}%`;
const LIT = new Set(["lit", "half"]);

export function Tape({ reading, runId, pointed }) {
  const { records, state, total, scan } = reading;
  const current = new Set(pointed);
  const delay = (record) => `${(reach(record.at / Math.max(total, 1)) * SCAN_SECONDS).toFixed(3)}s`;
  const has = (s) => state.includes(s);
  const scanned = Boolean(scan?.matches);
  const lit = state.filter((s) => LIT.has(s)).length;

  return (
    <div class="vc-tape">
      <div class="vc-map">
        <ul class="legend" aria-label="The tape">
          {has("lit") ? <li class="legend-item vc-key-lit">matched</li> : null}
          {has("half") ? <li class="legend-item vc-key-half">matched, dropped by Rust</li> : null}
          {has("tested") ? <li class="legend-item vc-key-tested">tested, rejected</li> : null}
          <li class="legend-item vc-key-plain">{scanned ? "not a candidate" : "record"}</li>
        </ul>
        <div class="vc-brackets" aria-hidden="true">
          {regions(records).map((region) => (
            <span
              key={region.at}
              style={{ left: pct(region.at, total), width: `calc(${pct(region.end - region.at, total)} - 2px)` }}
            >
              <b>{region.label}</b>
            </span>
          ))}
        </div>
        <div
          key={runId}
          class="vc-track"
          role="img"
          aria-label={
            scanned
              ? `${lit} of ${records.length} records matched, ${reading.litBytes} of ${total} bytes`
              : `${records.length} records, ${total} bytes`
          }
        >
          {records.map((record, index) => (
            <span
              key={record.at}
              class={`vc-rec is-${state[index]}${current.has(index) ? " is-current" : ""}`}
              style={{ flexGrow: record.end - record.at, "--d": delay(record) }}
            />
          ))}
        </div>
        <div class="vc-ruler" aria-hidden="true">
          {ticks(total).map((at) => (
            <Fragment key={at}>
              <i style={{ left: pct(at, total) }} />
              <span class={at ? "is-mid" : ""} style={{ left: pct(at, total) }}>
                {at}
              </span>
            </Fragment>
          ))}
          <i style={{ left: "calc(100% - 1px)" }} />
          <span class="is-end">{total.toLocaleString()} B</span>
        </div>
      </div>
    </div>
  );
}
