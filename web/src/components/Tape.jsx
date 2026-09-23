// The whole database drawn as a paper tape: a byte map with one span per
// record, and the raw string under it. The read head crosses the map once
// per run and lights each record it matched as it gets there.

import { Fragment } from "preact";

import { CopyButton, Icon } from "./ui.jsx";
import { reach, regions, ticks } from "../lib/tape.js";

/** Seconds the head takes to cross the tape. Mirrors --vc-scan in app.css. */
const SCAN_SECONDS = 1.6;
/** Past this many marks the rest of the string stays plain text. */
const MARK_LIMIT = 600;

const pct = (n, total) => `${((n / total) * 100).toFixed(3)}%`;
const LIT = new Set(["lit", "half"]);

export function Tape({ reading, runId, pointed, live, historic, onHistoric, blob, onSave, onDrop }) {
  const { records, state, total, scan } = reading;
  const current = new Set(pointed);
  const delay = (record) => `${(reach(record.at / Math.max(total, 1)) * SCAN_SECONDS).toFixed(3)}s`;
  const has = (s) => state.includes(s);
  const scanned = Boolean(scan?.matches);
  const parkAt = pointed.length ? records[pointed[0]] : records[state.findIndex((s) => LIT.has(s))];

  // Consecutive unlit records share one text node; each lit one is a mark.
  const pieces = [];
  let plain = "";
  let marks = 0;
  records.forEach((record, index) => {
    if (LIT.has(state[index]) && marks < MARK_LIMIT) {
      if (plain) pieces.push(plain);
      plain = "";
      marks += 1;
      const classes = ["vc-lit", state[index] === "half" && "is-half", current.has(index) && "is-current"];
      pieces.push(
        <mark key={record.at} class={classes.filter(Boolean).join(" ")} style={{ "--d": delay(record) }}>
          {record.text}
        </mark>,
      );
    } else {
      plain += record.text;
    }
  });
  if (plain) pieces.push(plain);

  const lit = state.filter((s) => LIT.has(s)).length;

  return (
    <section class="terminal vc-tape" aria-labelledby="tape-heading">
      <div class="vc-map">
        <div class="vc-map-head">
          <h2 class="cn-name cn-m-0" id="tape-heading">
            The tape
          </h2>
          <ul class="legend">
            {has("lit") ? <li class="legend-item cn-tone-mauve">matched</li> : null}
            {has("half") ? <li class="legend-item vc-key-half">matched, dropped by Rust</li> : null}
            {has("tested") ? <li class="legend-item vc-key-tested">tested, rejected</li> : null}
            <li class="legend-item vc-key-plain">not a candidate</li>
          </ul>
          <div class="vc-map-actions">
            {onHistoric ? (
              <button
                type="button"
                class="btn btn-ghost is-sm"
                aria-pressed={String(historic)}
                title="Show the string the scan read, with the rows it matched"
                onClick={onHistoric}
              >
                before the write
              </button>
            ) : null}
            <CopyButton text={blob} icon="copy" />
            <button type="button" class="btn btn-ghost is-sm" onClick={onSave}>
              <Icon id="download" /> save
            </button>
            <button type="button" class="btn btn-ghost is-sm is-danger" onClick={onDrop}>
              <Icon id="trash" /> drop all
            </button>
          </div>
        </div>
        {total > 0 ? (
          <>
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
              class={`vc-track${live ? " is-live" : ""}`}
              role="img"
              aria-label={`${lit} of ${records.length} records matched, ${reading.litBytes} of ${total} bytes`}
            >
              {records.map((record, index) => (
                <span
                  key={record.at}
                  class={`vc-rec is-${state[index]}${current.has(index) ? " is-current" : ""}`}
                  style={{ flexGrow: record.end - record.at, "--d": delay(record) }}
                />
              ))}
              {scanned ? <span class="vc-head" aria-hidden="true" /> : null}
              {scanned && parkAt ? (
                <span class="vc-park" aria-hidden="true" style={{ "--x": pct(parkAt.at, total) }} />
              ) : null}
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
          </>
        ) : null}
      </div>
      <pre key={runId} class="scroll-well" role="region" tabindex={0} aria-label="The encoded database string">
        {total === 0 ? <span class="vc-null">(empty)</span> : pieces}
      </pre>
    </section>
  );
}
