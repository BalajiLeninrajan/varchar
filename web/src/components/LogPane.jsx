import { useEffect, useRef } from "preact/hooks";

import { Pane, PaneHead } from "./ui.jsx";

export function LogPane({ entries, leaving, onClear, onClose }) {
  const stream = useRef(null);

  useEffect(() => {
    if (stream.current) stream.current.scrollTop = stream.current.scrollHeight;
  }, [entries]);

  return (
    <Pane className={`log-pane${leaving ? " is-leaving" : ""}`} aria-labelledby="log-heading">
      <PaneHead title="log" id="log-heading">
        <button class="btn-flat" onClick={onClear}>
          clear
        </button>
        <button class="btn-flat" onClick={onClose}>
          hide
        </button>
      </PaneHead>
      <div class="pane-body is-flush">
        <pre ref={stream} class="scroll-well" aria-live="polite">
          {entries.map((entry) => (
            <span key={entry.id} class={`log-line${entry.tone === "err" ? " is-error" : ""}`}>
              <span class="log-time">{entry.time} </span>
              <span class={entry.statement ? "log-sql" : `log-${entry.tone}`}>{entry.text}</span>
              {entry.detail ? (
                <span class={`log-${entry.tone}`}>{`\n         ${entry.detail}`}</span>
              ) : null}
              {"\n"}
            </span>
          ))}
        </pre>
      </div>
    </Pane>
  );
}
