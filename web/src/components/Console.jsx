import { useRef } from "preact/hooks";

import { Icon, Pane, PaneHead } from "./ui.jsx";

export function Console({ sql, onSql, onRun, onSeed }) {
  const field = useRef(null);

  return (
    <Pane className="console-pane" aria-labelledby="console-heading">
      <PaneHead title="console" id="console-heading">
        <button
          class="btn-flat"
          onClick={() => {
            onSql("");
            field.current?.focus();
          }}
        >
          clear
        </button>
        <span class="hint">⌘/ctrl + ⏎</span>
      </PaneHead>
      <div class="pane-body console-body">
        <textarea
          ref={field}
          id="sql"
          name="sql"
          value={sql}
          spellcheck={false}
          autocapitalize="off"
          autocorrect="off"
          aria-label="SQL statements, separated by semicolons"
          placeholder="SELECT name FROM users WHERE active = TRUE"
          onInput={(event) => onSql(event.currentTarget.value)}
          onKeyDown={(event) => {
            if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
              event.preventDefault();
              onRun();
            }
          }}
        />
        <div class="button-row is-end">
          <button class="btn-ghost" onClick={onSeed}>
            seed demo data
          </button>
          <button class="btn-primary" onClick={onRun}>
            <Icon id="play" size={11} /> run
          </button>
        </div>
      </div>
    </Pane>
  );
}
