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
    <header class="appbar topbar is-compact is-split">
      <span class="wordmark">
        <span class="mark">
          <Icon id="mark" size={13} />
        </span>
        var<em>char</em>
      </span>
      <div class="cn-row cn-wrap cn-end cn-gap-4 cn-min-0">
        <Chip class="cn-gap-4" title="Length of the encoded string in bytes">
          <b>{stats.bytes.toLocaleString()}</b>B
        </Chip>
        <Chip class="cn-gap-4" title="~S records in the string">
          <b>{stats.tables}</b>tbl
        </Chip>
        <Chip class="cn-gap-4" title="~R records in the string">
          <b>{stats.rows}</b>rows
        </Chip>
        <button class="btn btn-ghost is-sm" onClick={onOpenPresets}>
          examples
        </button>
        <button class="btn btn-ghost is-sm" onClick={onOpenReference}>
          sql
        </button>
        <button class="btn btn-ghost is-sm" onClick={onOpenImport}>
          import
        </button>
        <button
          class="btn btn-ghost is-sm"
          aria-expanded={String(logOpen)}
          aria-controls="log-pane"
          onClick={onToggleLog}
        >
          log
        </button>
        <button class="btn btn-ghost is-sm" onClick={onOpenAbout}>
          about
        </button>
        {LINKS.map((link) => (
          <a
            key={link.id}
            class="btn btn-ghost is-sm"
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
