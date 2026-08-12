import { Chip, Icon } from "./ui.jsx";

const LINKS = [
  {
    id: "github",
    href: "https://github.com/BalajiLeninrajan/varchar",
    title: "GitHub",
  },
  {
    id: "crate",
    href: "https://crates.io/crates/varchar",
    title: "crates.io/varchar",
  },
  { id: "book", href: "https://docs.rs/varchar", title: "docs.rs/varchar" },
];

export function Topbar({
  stats,
  logOpen,
  onToggleLog,
  onOpenPresets,
  onOpenReference,
  onOpenImport,
  onOpenAbout,
}) {
  return (
    <header class="topbar">
      <div class="topbar-start">
        <span class="mark-solid">
          <Icon id="mark" size={13} />
        </span>
        <span class="wordmark">
          var<em>char</em>
        </span>
      </div>
      <div class="topbar-end">
        <Chip title="Length of the encoded string in bytes">
          <b>{stats.bytes.toLocaleString()}</b>&nbsp;B
        </Chip>
        <Chip title="~S records in the string">
          <b>{stats.tables}</b>&nbsp;tbl
        </Chip>
        <Chip title="~R records in the string">
          <b>{stats.rows}</b>&nbsp;rows
        </Chip>
        <button class="btn-flat" onClick={onOpenPresets}>
          examples
        </button>
        <button class="btn-flat" onClick={onOpenReference}>
          sql
        </button>
        <button class="btn-flat" onClick={onOpenImport}>
          import
        </button>
        <button
          class={`btn-flat${logOpen ? " is-on" : ""}`}
          aria-expanded={String(logOpen)}
          onClick={onToggleLog}
        >
          log
        </button>
        <button class="btn-flat" onClick={onOpenAbout}>
          about
        </button>
        {LINKS.map((link) => (
          <a
            key={link.id}
            class="btn-flat"
            href={link.href}
            title={link.title}
            target="_blank"
            rel="noreferrer noopener"
          >
            <Icon id={link.id} size={13} />
          </a>
        ))}
      </div>
    </header>
  );
}
