import { useEffect, useMemo, useRef, useState } from "preact/hooks";

import { CopyCommand, EmptyState, Icon, Marked, Modal } from "./ui.jsx";
import { GROUPS } from "../lib/presets.js";
import { SECTIONS } from "../lib/reference.js";
import { tokenizeSql } from "../lib/sql.js";

export function AboutDialog({ open, onClose }) {
  return (
    <Modal open={open} onClose={onClose} className="modal is-wide sheet is-about">
      <div class="sheet-body">
        <h1 class="display-title is-sm cn-mb-16">
          The whole database is <em>one string</em>.
        </h1>
        <p class="lede cn-mt-0 cn-mb-24">
          Schemas, constraints, sequence state and every row live in a single UTF-8 <code class="cn-code-inline">String</code>, and
          every <code class="cn-code-inline">SELECT</code> is a regular expression scanned across it. This page runs the real engine
          compiled to WebAssembly: nothing leaves your tab, and nothing survives a reload.
        </p>
        <pre class="sample codeblock cn-mt-0 cn-mb-24">
          <span class="tok-tag">V2;</span>
          {"\n"}
          <span class="tok-tag">~S|</span>
          <span class="tok-name">users</span>
          <span class="tok-tag">|</span>id:I:!<span class="tok-tag">|</span>name:T:?<span class="tok-tag">|</span>
          active:B:?<span class="tok-tag">;</span>
          {"\n"}
          <span class="tok-tag">~P|</span>
          <span class="tok-name">users</span>
          <span class="tok-tag">|</span>id<span class="tok-tag">;</span>
          {"\n"}
          <span class="tok-tag">~A|</span>
          <span class="tok-name">users</span>
          <span class="tok-tag">|</span>id<span class="tok-tag">|</span>
          <span class="tok-cell">I1</span>
          <span class="tok-tag">;</span>
          {"\n"}
          <span class="tok-tag">~R|</span>
          <span class="tok-name">users</span>
          <span class="tok-tag">|</span>
          <span class="tok-cell">I1</span>
          <span class="tok-tag">|</span>
          <span class="tok-cell">TAda</span>
          <span class="tok-tag">|</span>
          <span class="tok-cell">B1</span>
          <span class="tok-tag">;</span>
        </pre>
        <ul class="about-list cn-mb-24">
          <li>
            <b>Run</b> anything in the input at the top. Statements are split on <code class="cn-code-inline">;</code> and executed one at a
            time.
          </li>
          <li>
            Every <code class="cn-code-inline">SELECT</code> shows the pattern it compiled to, and lights the records it matched on
            the tape.
          </li>
          <li>The string is the whole database. Copy it, save it, or import one back.</li>
        </ul>
        <div class="cn-grid-2">
          <CopyCommand command="cargo add varchar" />
          <CopyCommand command="cargo install varchar-cli" />
        </div>
      </div>
      {/* Outside the scrolling body so the CTA is reachable on a short screen. */}
      <footer class="sheet-foot panel-footer">
        <nav class="cn-cluster cn-gap-24">
          <a class="btn-text" href="https://github.com/BalajiLeninrajan/varchar" target="_blank" rel="noreferrer noopener">
            <Icon id="github" size={13} /> GitHub
          </a>
          <a class="btn-text" href="https://crates.io/crates/varchar" target="_blank" rel="noreferrer noopener">
            <Icon id="crate" size={13} /> crates.io
          </a>
          <a class="btn-text" href="https://crates.io/crates/varchar-cli" target="_blank" rel="noreferrer noopener">
            <Icon id="crate" size={13} /> varchar-cli
          </a>
          <a class="btn-text" href="https://docs.rs/varchar" target="_blank" rel="noreferrer noopener">
            <Icon id="book" size={13} /> docs.rs
          </a>
        </nav>
        <button class="btn btn-primary is-lg" onClick={onClose}>
          close
        </button>
      </footer>
    </Modal>
  );
}

export function PresetsDrawer({ open, onClose, onPick }) {
  const [active, setActive] = useState(null);
  return (
    <Modal open={open} onClose={onClose} className="drawer side-sheet">
      <header class="panel-header">
        <h2>Examples</h2>
        <button class="btn btn-ghost is-sm" onClick={onClose}>
          close
        </button>
      </header>
      <div class="drawer-body scroll-well">
        {GROUPS.map((group) => (
          <div class="cn-stack cn-gap-8" key={group.title}>
            <h3 class="cn-ui cn-m-0">{group.title}</h3>
            <div class="preset-list segmented is-stacked cn-gap-8">
              {group.presets.map((preset) => (
                <button
                  key={preset.id}
                  type="button"
                  aria-pressed={String(active === preset.id)}
                  onClick={() => {
                    setActive(preset.id);
                    onPick(preset);
                  }}
                >
                  <b><Marked text={preset.name} /></b>
                  <small>{preset.blurb}</small>
                </button>
              ))}
            </div>
          </div>
        ))}
      </div>
    </Modal>
  );
}

const REFERENCE_DOC =
  "https://github.com/BalajiLeninrajan/varchar/blob/main/docs/sql-reference.md";

/** SQL with the dialect's keywords, types and literals coloured. */
function Sql({ text }) {
  return tokenizeSql(text).map((token, index) => (
    <span key={index} class={`sql-${token.kind}`}>
      {token.text}
    </span>
  ));
}

/** Sections whose title matches keep every entry; otherwise entries filter. */
function search(query) {
  const needle = query.trim().toLowerCase();
  if (!needle) return SECTIONS;
  return SECTIONS.map((section) => {
    if (section.title.toLowerCase().includes(needle)) return section;
    const entries = (section.entries ?? []).filter((entry) =>
      `${entry.syntax} ${entry.note}`.toLowerCase().includes(needle),
    );
    const items = (section.items ?? []).filter((item) => item.toLowerCase().includes(needle));
    if (entries.length === 0 && items.length === 0) return null;
    return { ...section, entries, items };
  }).filter(Boolean);
}

export function ReferenceDrawer({ open, onClose, onUse }) {
  const [query, setQuery] = useState("");
  const body = useRef(null);
  const sections = useMemo(() => search(query), [query]);

  // The drawer stays mounted while closed, so each opening resets it to the
  // top of the whole grammar rather than wherever it was left.
  useEffect(() => {
    if (!open) return;
    setQuery("");
    if (body.current) body.current.scrollTop = 0;
  }, [open]);

  return (
    <Modal open={open} onClose={onClose} className="drawer side-sheet is-wide">
      <header class="panel-header">
        <h2>SQL reference</h2>
        <button class="btn btn-ghost is-sm" onClick={onClose}>
          close
        </button>
      </header>
      <div class="drawer-search">
        <input
          type="search"
          name="ref-search"
          class="input"
          value={query}
          spellcheck={false}
          autocapitalize="off"
          autocorrect="off"
          aria-label="Filter the SQL reference"
          placeholder="filter: like, cascade, order by"
          onInput={(event) => setQuery(event.currentTarget.value)}
        />
      </div>
      <div class="drawer-body scroll-well" ref={body}>
        {sections.length === 0 ? (
          <EmptyState title="Nothing matches">
            No clause in the dialect mentions that. The whole grammar is on the other side of the filter.
          </EmptyState>
        ) : null}
        {sections.map((section) => (
          <section class="cn-stack cn-gap-8" key={section.title}>
            <h3 class="cn-ui cn-m-0">{section.title}</h3>
            {section.blurb ? <p class="cn-meta cn-m-0">{section.blurb}</p> : null}
            {section.entries?.length ? (
              <ul class="cn-list-none cn-stack cn-gap-8">
                {section.entries.map((entry) => (
                  <li key={entry.syntax} class="cn-stack cn-gap-4 cn-mt-0">
                    {/* A complete statement carries a button that loads the
                        console; a fragment is shown for its shape alone. */}
                    <div class="command is-full is-wrap">
                      <code class="command-text">
                        <Sql text={entry.syntax} />
                      </code>
                      {entry.run ? (
                        <button
                          type="button"
                          class="btn-text cn-fixed"
                          title="Put this in the console"
                          onClick={() => onUse(entry.syntax)}
                        >
                          use
                        </button>
                      ) : null}
                    </div>
                    <p class="cn-meta cn-m-0">{entry.note}</p>
                  </li>
                ))}
              </ul>
            ) : null}
            {section.items?.length ? (
              <ul class="cn-list-none cn-cluster cn-gap-4">
                {section.items.map((item) => (
                  <li key={item} class="cn-edge-dashed cn-r-pill cn-meta cn-px-8 cn-mt-0">
                    {item}
                  </li>
                ))}
              </ul>
            ) : null}
            {section.note ? <p class="cn-meta cn-m-0">{section.note}</p> : null}
          </section>
        ))}
      </div>
      <footer class="panel-footer">
        <span class="cn-meta">The dialect is small on purpose.</span>
        <a class="btn-text" href={REFERENCE_DOC} target="_blank" rel="noreferrer noopener">
          <Icon id="book" size={13} /> full reference
        </a>
      </footer>
    </Modal>
  );
}

export function ImportDialog({ open, onClose, onLoadBlob, onImportCsv }) {
  const [text, setText] = useState("");
  const blobFile = useRef(null);
  const csvFile = useRef(null);

  const read = (input, handler) => {
    const file = input.files && input.files[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => handler(String(reader.result), file.name);
    reader.readAsText(file);
    input.value = "";
  };

  return (
    <Modal open={open} onClose={onClose} className="modal sheet">
      <header class="panel-header">
        <h2>Import</h2>
        <button class="btn btn-ghost is-sm" onClick={onClose}>
          close
        </button>
      </header>
      <div class="sheet-body cn-stack cn-gap-16">
        <div class="field">
          <label for="blob-input">paste an encoded database string</label>
          <textarea
            id="blob-input"
            class="is-code"
            spellcheck={false}
            value={text}
            placeholder="V2;~S|users|id:I:!|name:T:?;~P|users|id;"
            onInput={(event) => setText(event.currentTarget.value)}
          />
        </div>
        <div class="cn-row cn-wrap cn-end">
          <button class="btn btn-secondary" onClick={() => blobFile.current?.click()}>
            <Icon id="upload" /> open .varchar file
          </button>
          <button class="btn btn-primary" disabled={!text.trim()} onClick={() => onLoadBlob(text)}>
            load string
          </button>
        </div>
        <p class="cn-meta">
          Or turn a CSV into a table. Column types are inferred, then a <code class="cn-code-inline">CREATE TABLE</code> and one{" "}
          <code class="cn-code-inline">INSERT</code> per row are run for you.
        </p>
        <div class="cn-row cn-wrap cn-end">
          <button class="btn btn-secondary" onClick={() => csvFile.current?.click()}>
            <Icon id="upload" /> import CSV as a table
          </button>
        </div>
      </div>
      <input
        ref={blobFile}
        type="file"
        accept=".varchar,.txt,text/plain"
        class="cn-sr-only"
        onChange={(event) => read(event.currentTarget, (content) => onLoadBlob(content))}
      />
      <input
        ref={csvFile}
        type="file"
        accept=".csv,.tsv,text/csv"
        class="cn-sr-only"
        onChange={(event) => read(event.currentTarget, (content, name) => onImportCsv(content, name))}
      />
    </Modal>
  );
}
