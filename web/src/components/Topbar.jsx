import { Icon } from "./ui.jsx";

export function Topbar({ logOpen, onToggleLog, onOpenReference, onOpenImport, onOpenAbout }) {
  return (
    <header class="topbar is-compact is-split">
      <span class="wordmark">
        <span class="mark">
          <Icon id="mark" size={13} />
        </span>
        var<em>char</em>
      </span>
      <nav class="cn-row cn-wrap cn-end cn-gap-4 cn-min-0" aria-label="Playground">
        <button type="button" class="btn btn-ghost is-sm" onClick={onOpenReference}>
          sql
        </button>
        <button type="button" class="btn btn-ghost is-sm" onClick={onOpenImport}>
          import
        </button>
        <button
          type="button"
          class="btn btn-ghost is-sm"
          aria-expanded={String(logOpen)}
          aria-controls="log-pane"
          onClick={onToggleLog}
        >
          log
        </button>
        <button type="button" class="btn btn-ghost is-sm" onClick={onOpenAbout}>
          about
        </button>
      </nav>
    </header>
  );
}

export function Footer() {
  return (
    <footer class="page-footer">
      <span class="wordmark is-sm">
        <Icon id="mark" size={14} />
        varchar <span>the real engine, compiled to WebAssembly. Nothing leaves your tab.</span>
      </span>
      <span class="cn-row cn-gap-16">
        <a href="https://github.com/BalajiLeninrajan/varchar" target="_blank" rel="noreferrer noopener">
          GitHub
        </a>
        <a href="https://crates.io/crates/varchar" target="_blank" rel="noreferrer noopener">
          crates.io
        </a>
        <a href="https://docs.rs/varchar" target="_blank" rel="noreferrer noopener">
          docs.rs
        </a>
      </span>
    </footer>
  );
}
