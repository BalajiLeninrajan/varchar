import { Icon } from "./ui.jsx";

export const EXAMPLES = [
  { label: "active = TRUE", sql: "SELECT name, email FROM users WHERE active = TRUE" },
  { label: "LIKE 'A%'", sql: "SELECT name FROM users WHERE name LIKE 'A%'" },
  { label: "views > 1000", sql: "SELECT title, views FROM posts WHERE views > 1000" },
  { label: "a join", sql: "SELECT users.name, posts.title FROM users JOIN posts ON users.id = posts.user_id" },
];

/** The one input: a statement, the run button, and the examples to try. */
export function Ask({ sql, onSql, onRun, hasDemo, onSeed, onExample, onMore }) {
  return (
    <form
      class="vc-ask"
      onSubmit={(event) => {
        event.preventDefault();
        onRun();
      }}
    >
      <div class="vc-ask-row">
        <textarea
          id="sql"
          name="sql"
          rows={1}
          class="input vc-sql"
          value={sql}
          spellcheck={false}
          autocapitalize="off"
          autocorrect="off"
          autocomplete="off"
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
        <button class="btn btn-primary" type="submit" title="Run (⌘ or ctrl + Enter)">
          <Icon id="play" size={11} /> run
        </button>
      </div>
      <div class="cn-cluster cn-gap-4 cn-mt-12" role="group" aria-label="Examples">
        <span class="cn-label">or try</span>
        {hasDemo ? (
          EXAMPLES.map((example) => (
            <button
              key={example.label}
              type="button"
              class="btn btn-ghost is-sm"
              aria-pressed={String(sql.trim() === example.sql)}
              onClick={() => onExample(example.sql)}
            >
              {example.label}
            </button>
          ))
        ) : (
          <button type="button" class="btn btn-ghost is-sm" onClick={onSeed}>
            seed demo data
          </button>
        )}
        <button type="button" class="btn btn-ghost is-sm" onClick={onMore}>
          more
        </button>
      </div>
    </form>
  );
}
