import { useEffect, useMemo, useRef, useState } from "preact/hooks";

import { CopyCommand, EmptyState, Icon, Modal } from "./ui.jsx";
import { GROUPS } from "../lib/presets.js";
import { SECTIONS } from "../lib/reference.js";
import { tokenizeSql } from "../lib/sql.js";

export function AboutDialog({ open, onClose }) {
  return (
    <Modal open={open} onClose={onClose} className="sheet is-about">
      <div class="sheet-body">
        <p class="eyebrow">a really dumb SQL database</p>
        <h1 class="display-title">
          The whole database is <em>one string</em>.
        </h1>
        <p class="lede">
          Schemas, constraints, sequence state and every row live in a single UTF-8 <code>String</code>, and
          every <code>SELECT</code> is a regular expression scanned across it. This page runs the real engine
          compiled to WebAssembly: nothing leaves your tab, and nothing survives a reload.
        </p>
        <pre class="sample well">
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
        <ul class="about-list">
          <li>
            <b>Run</b> anything in the console. Statements are split on <code>;</code> and executed one at a
            time.
          </li>
          <li>
            Every <code>SELECT</code> shows the pattern it compiled to, and highlights the bytes it matched in
            the string at the bottom.
          </li>
          <li>The string is the whole database. Copy it, save it, or import one back.</li>
        </ul>
        <div class="install">
          <CopyCommand command="cargo add varchar" />
          <CopyCommand command="cargo install varchar-cli" />
        </div>
      </div>
      {/* Outside the scrolling body so the CTA is reachable on a short screen. */}
      <footer class="sheet-foot">
          <nav class="about-links">
            <a href="https://github.com/BalajiLeninrajan/varchar" target="_blank" rel="noreferrer noopener">
              <Icon id="github" size={13} /> GitHub
            </a>
            <a href="https://crates.io/crates/varchar" target="_blank" rel="noreferrer noopener">
              <Icon id="crate" size={13} /> crates.io
            </a>
            <a href="https://crates.io/crates/varchar-cli" target="_blank" rel="noreferrer noopener">
              <Icon id="crate" size={13} /> varchar-cli
            </a>
            <a href="https://docs.rs/varchar" target="_blank" rel="noreferrer noopener">
              <Icon id="book" size={13} /> docs.rs
            </a>
          </nav>
          <button class="btn-primary" onClick={onClose}>
            start
          </button>
      </footer>
    </Modal>
  );
}

export function PresetsDrawer({ open, onClose, onPick }) {
  const [active, setActive] = useState(null);
  return (
    <Modal open={open} onClose={onClose} className="drawer">
      <div class="pane-head">
        <h2>examples</h2>
        <button class="btn-flat" onClick={onClose}>
          close
        </button>
      </div>
      <div class="drawer-body scroll-well">
        {GROUPS.map((group) => (
          <div class="drawer-group" key={group.title}>
            <h3>{group.title}</h3>
            <div class="preset-list">
              {group.presets.map((preset) => (
                <button
                  key={preset.id}
                  type="button"
                  class={active === preset.id ? "active" : ""}
                  onClick={() => {
                    setActive(preset.id);
                    onPick(preset);
                  }}
                >
                  <b>{preset.name}</b>
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
    <Modal open={open} onClose={onClose} className="drawer is-wide">
      <div class="pane-head">
        <h2>sql reference</h2>
        <button class="btn-flat" onClick={onClose}>
          close
        </button>
      </div>
      <div class="drawer-search">
        <input
          type="search"
          value={query}
          spellcheck={false}
          autocapitalize="off"
          autocorrect="off"
          aria-label="Filter the SQL reference"
          placeholder="filter — like, cascade, order by"
          onInput={(event) => setQuery(event.currentTarget.value)}
        />
      </div>
      <div class="drawer-body scroll-well" ref={body}>
        {sections.length === 0 ? (
          <EmptyState title="nothing matches">
            No clause in the dialect mentions that. The whole grammar is on the other side of the filter.
          </EmptyState>
        ) : null}
        {sections.map((section) => (
          <section class="drawer-group" key={section.title}>
            <h3>{section.title}</h3>
            {section.blurb ? <p class="ref-blurb">{section.blurb}</p> : null}
            {section.entries?.length ? (
              <ul class="ref-list">
                {section.entries.map((entry) => (
                  <li key={entry.syntax}>
                    {/* A complete statement is a button that loads the console;
                        a fragment is shown for its shape and nothing else. */}
                    {entry.run ? (
                      <button
                        type="button"
                        class="ref-syntax is-runnable"
                        title="Put this in the console"
                        onClick={() => onUse(entry.syntax)}
                      >
                        <code>
                          <Sql text={entry.syntax} />
                        </code>
                        <span class="ref-use">use</span>
                      </button>
                    ) : (
                      <div class="ref-syntax">
                        <code>
                          <Sql text={entry.syntax} />
                        </code>
                      </div>
                    )}
                    <p class="ref-note">{entry.note}</p>
                  </li>
                ))}
              </ul>
            ) : null}
            {section.items?.length ? (
              <ul class="ref-tags">
                {section.items.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            ) : null}
            {section.note ? <p class="ref-blurb is-foot">{section.note}</p> : null}
          </section>
        ))}
      </div>
      <footer class="drawer-foot">
        <span>The dialect is small on purpose.</span>
        <a href={REFERENCE_DOC} target="_blank" rel="noreferrer noopener">
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
    <Modal open={open} onClose={onClose} className="sheet is-narrow">
      <div class="pane-head">
        <h2>import</h2>
        <button class="btn-flat" onClick={onClose}>
          close
        </button>
      </div>
      <div class="sheet-body">
        <div class="field">
          <label for="blob-input">paste an encoded database string</label>
          <textarea
            id="blob-input"
            spellcheck={false}
            value={text}
            placeholder="V2;~S|users|id:I:!|name:T:?;~P|users|id;"
            onInput={(event) => setText(event.currentTarget.value)}
          />
        </div>
        <div class="button-row is-end">
          <button class="btn-secondary" onClick={() => blobFile.current?.click()}>
            <Icon id="upload" /> open .varchar file
          </button>
          <button class="btn-primary" disabled={!text.trim()} onClick={() => onLoadBlob(text)}>
            load string
          </button>
        </div>
        <p class="note">
          Or turn a CSV into a table — column types are inferred, then a <code>CREATE TABLE</code> and one{" "}
          <code>INSERT</code> per row are run for you.
        </p>
        <div class="button-row is-end">
          <button class="btn-secondary" onClick={() => csvFile.current?.click()}>
            <Icon id="upload" /> import CSV as a table
          </button>
        </div>
      </div>
      <input
        ref={blobFile}
        type="file"
        accept=".varchar,.txt,text/plain"
        class="sr-only"
        onChange={(event) => read(event.currentTarget, (content) => onLoadBlob(content))}
      />
      <input
        ref={csvFile}
        type="file"
        accept=".csv,.tsv,text/csv"
        class="sr-only"
        onChange={(event) => read(event.currentTarget, (content, name) => onImportCsv(content, name))}
      />
    </Modal>
  );
}
