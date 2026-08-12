import { useEffect, useMemo, useRef } from "preact/hooks";

import { CopyButton, Icon, Pane } from "./ui.jsx";
import { encode, segmentMatches } from "../lib/bytes.js";

const MARK_LIMIT = 600;

export function StringDock({
  blob,
  scan,
  blobBefore,
  explain,
  onExplain,
  current,
  onCurrent,
  open,
  onToggle,
  onSave,
  onDrop,
}) {
  const marks = useRef([]);

  // A mutation's ranges index the string it scanned, which is the one from
  // before the write — so lighting them up means showing that string too. If
  // that string is missing the ranges describe nothing on screen, and drawing
  // them over the live blob would be worse than drawing nothing.
  const beforeWrite = scan?.appliesTo === "before";
  const historic = beforeWrite && typeof blobBefore === "string";
  const shown = historic && explain ? blobBefore : blob;
  const matches = explain && (historic || !beforeWrite) ? (scan?.matches ?? []) : [];

  const bytes = useMemo(() => encode(shown), [shown]);
  const { segments, drawn } = useMemo(
    () => segmentMatches(bytes, matches, MARK_LIMIT),
    [bytes, matches],
  );

  useEffect(() => {
    marks.current[current]?.scrollIntoView({
      block: "nearest",
      inline: "nearest",
    });
  }, [current, segments]);

  const total = matches.length;
  const step = (delta) => onCurrent((current + delta + drawn) % drawn);

  return (
    <Pane
      className={`dock${open ? "" : " is-collapsed"}`}
      aria-labelledby="dock-heading"
    >
      <div class="pane-head">
        <h2 id="dock-heading">
          the database string
          {explain && historic ? (
            <em class="as-of"> · before the write</em>
          ) : null}
        </h2>
        <div class="head-chips">
          <span class="note" id="blob-note">
            {total !== 0 && (
              <>
                <span class="swatch" />
                {total.toLocaleString()} byte range{total === 1 ? "" : "s"}{" "}
                matched
                {drawn < total
                  ? ` · first ${drawn.toLocaleString()} shown`
                  : ""}
              </>
            )}
          </span>
          {scan?.matches?.length ? (
            <button
              class={`btn-flat${explain ? " is-on" : ""}`}
              aria-pressed={String(explain)}
              title={
                historic
                  ? "Show the string the scan read, with the rows it matched"
                  : "Highlight the bytes the pattern matched"
              }
              onClick={onExplain}
            >
              explain
            </button>
          ) : null}
          {drawn > 0 ? (
            <span class="match-nav">
              <button
                class="btn-flat"
                onClick={() => step(-1)}
                title="Previous match"
                aria-label="Previous match"
              >
                <Icon id="left" />
              </button>
              <output>
                {current + 1} / {drawn}
              </output>
              <button
                class="btn-flat"
                onClick={() => step(1)}
                title="Next match"
                aria-label="Next match"
              >
                <Icon id="right" />
              </button>
            </span>
          ) : null}
          <span class="head-rule" />
          <button class="btn-flat is-danger" onClick={onDrop}>
            <Icon id="trash" /> drop all
          </button>
          <span class="head-rule" />
          <CopyButton text={blob} icon="copy" />
          <button class="btn-flat" onClick={onSave}>
            <Icon id="download" /> save
          </button>
          <button
            class="btn-flat"
            aria-expanded={String(open)}
            title={open ? "Collapse" : "Expand"}
            onClick={onToggle}
          >
            <Icon id="chevron" size={13} />
          </button>
        </div>
      </div>
      <div class="collapsible">
        <div class="pane-body is-flush">
          <pre
            class="scroll-well"
            aria-live="polite"
            aria-label="The encoded database string"
          >
            {bytes.length === 0 ? (
              <span class="blob-empty">(empty)</span>
            ) : null}
            {segments.map((segment, index) =>
              segment.match === -1 ? (
                segment.text
              ) : (
                <mark
                  key={index}
                  ref={(element) => {
                    marks.current[segment.match] = element;
                  }}
                  class={segment.match === current ? "is-current" : ""}
                >
                  {segment.text}
                </mark>
              ),
            )}
          </pre>
        </div>
      </div>
    </Pane>
  );
}
